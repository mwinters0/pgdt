//! The flag groups and value parsers more than one subcommand reads.

use std::ffi::OsStr;
use std::fmt::Display;
use std::path::Path;
use std::str::FromStr;

use clap::builder::{PossibleValue, StringValueParser, TypedValueParser};
use clap::{Arg, Args, Command};
use pgdt_dump::cache::{CacheMode, StrictIdentity};
use pgdt_dump::{ReadOptions, ScanOptions};
use pgdt_io::Origin;

use crate::parallel::Resolved;

/// The cache a `--dtcache` names for `origin`: the default where it is
/// absent, none at all for the literal path `none` — `pgdt`'s spelling, the
/// library taking every path as given — and any other path as given.
pub(crate) fn cache_mode(origin: &Origin, dtcache: Option<&Path>) -> CacheMode {
    match dtcache {
        Some(path) if path == Path::new("none") => CacheMode::DISABLED,
        dtcache => CacheMode::resolve(origin, dtcache),
    }
}

/// A flag naming a library mode by the library's own words — its type's
/// `FromStr`, in any case — `--help` listing those of `modes` that `accept`
/// takes, by their `Display`. `accept` refuses, in the library's words too,
/// a mode the flag's command may not state.
#[derive(Clone, Copy)]
pub(crate) struct ModeParser<T: 'static> {
    modes: &'static [T],
    accept: fn(T) -> Result<T, String>,
}

impl<T> ModeParser<T> {
    /// A flag taking every one of `modes`.
    pub(crate) fn every(modes: &'static [T]) -> Self {
        Self { modes, accept: Ok }
    }

    /// A flag taking those of `modes` that `accept` does.
    pub(crate) fn accepting(modes: &'static [T], accept: fn(T) -> Result<T, String>) -> Self {
        Self { modes, accept }
    }
}

impl<T> TypedValueParser for ModeParser<T>
where
    T: Copy + Display + FromStr<Err = String> + Send + Sync + 'static,
{
    type Value = T;

    fn parse_ref(&self, cmd: &Command, arg: Option<&Arg>, value: &OsStr) -> Result<T, clap::Error> {
        let accept = self.accept;
        StringValueParser::new()
            .try_map(move |word| word.parse::<T>().and_then(accept))
            .parse_ref(cmd, arg, value)
    }

    fn possible_values(&self) -> Option<Box<dyn Iterator<Item = PossibleValue> + '_>> {
        let accepted = self.modes.iter().filter(|mode| (self.accept)(**mode).is_ok());
        Some(Box::new(accepted.map(|mode| PossibleValue::new(mode.to_string()))))
    }
}

/// Which weak identity signals a command asks to *bind*, shared by every
/// command that reads a dump against a cache
/// (`docs/design/decisions.md`, "D21").
#[derive(Args)]
pub(crate) struct IdentityArgs {
    /// Which weak signs of a source's identity must hold, as a comma-separated
    /// selection of `time` and `location`, or `advisory` or `none` alone; the
    /// flag alone is `time,location`. Left unstated, or stated `advisory`,
    /// they are advisory: a dump that was moved, copied or touched since it
    /// was parsed is read with a warning, because data moves and none of these
    /// signals proves the bytes changed.
    /// `time` binds the modification signal — an mtime, or a server's
    /// `Last-Modified` and `ETag` — so a cache written against another one, or
    /// a source that offers none at all, stops the run instead.
    /// `location` binds the origin a remote cache records, which a local file
    /// was not fetched from, so it binds nothing there.
    ///
    /// **A source that changes while it is being read is an error whatever
    /// this says**, because bytes moving underneath a read that has already
    /// returned some of them cannot produce a right answer; `none` is the only
    /// way to turn that into a warning, and it turns off everything else too.
    /// So a source nothing can check while it is read — a server stating
    /// neither a strong entity tag nor a `Last-Modified` — is refused unless
    /// this says `none`.
    ///
    /// It asks one question of every command, and needs a source to ask it
    /// of, so stating it without `--source` — `info` answering from the cache
    /// alone, whose identity is not unreadable but absent — is a usage error.
    #[arg(
        long,
        value_name = "TERMS",
        require_equals = true,
        num_args = 0..=1,
        default_missing_value = "time,location",
        requires = "source",
        value_parser = parse_strict_identity
    )]
    strict_identity: Option<StrictIdentity>,
}

impl IdentityArgs {
    /// What the library is told, which is the default where nothing was
    /// stated.
    pub(crate) fn resolve(&self) -> StrictIdentity {
        self.strict_identity.unwrap_or_default()
    }
}

/// Whether a scan may replace a cache it cannot use, shared by the two
/// commands that scan (`docs/design/decisions.md`, "D20").
#[derive(Args)]
pub(crate) struct OverwriteArgs {
    /// Start cold over a cache that cannot be used and replace it, rather
    /// than refusing: one written for a file of another size or compression,
    /// by another pgdt build, or cut short. Left unstated, each of those stops
    /// the run before the dump is read past its first bytes, since it is almost always the wrong
    /// path or a file that changed — this is for a source whose file is
    /// replaced under the same name. A file that is not a pgdt cache is never
    /// overwritten. It governs only the cache found at the start: a dump that
    /// changes while it is being read still stops the run
    /// (`--strict-identity`).
    #[arg(long)]
    pub(crate) overwrite_unusable_cache: bool,
}

/// A `--strict-identity` selection, in the library's one grammar
/// (`StrictIdentity`'s `FromStr`).
fn parse_strict_identity(text: &str) -> std::result::Result<StrictIdentity, String> {
    text.parse()
}

/// A `--chunk-size` value: a byte count, and never zero.
///
/// Zero is refused here rather than at the read loop, whose
/// `min(chunk_size, remaining)` would ask for nothing forever — a scan that
/// never advances and never errors.
pub(crate) fn parse_chunk_size(text: &str) -> std::result::Result<usize, String> {
    match text.parse::<usize>() {
        Ok(0) => Err("a chunk size of 0 would read nothing".to_string()),
        Ok(n) => Ok(n),
        Err(e) => Err(e.to_string()),
    }
}

/// A `--max-line-bytes` value: a byte count, and never zero.
///
/// Zero would refuse every line that crosses a read, which is every line of
/// a file longer than one chunk — a limit nobody means, so it is refused
/// here rather than reported as a line too long.
pub(crate) fn parse_max_line_bytes(text: &str) -> std::result::Result<usize, String> {
    match text.parse::<usize>() {
        Ok(0) => Err("a line limit of 0 would refuse every line a read splits".to_string()),
        Ok(n) => Ok(n),
        Err(e) => Err(e.to_string()),
    }
}

/// The two read flags every scanning command carries, as given.
#[derive(Clone, Copy)]
pub(crate) struct ReadFlags {
    pub(crate) chunk_size: Option<usize>,
    pub(crate) max_line_bytes: Option<usize>,
}

impl ReadFlags {
    /// The [`ReadOptions`] these flags state: the default, with
    /// `--chunk-size` and `--max-line-bytes` applied where they were given.
    pub(crate) fn options(self) -> ReadOptions {
        let default = ReadOptions::default();
        ReadOptions {
            chunk_size_bytes: self.chunk_size.unwrap_or(default.chunk_size_bytes),
            max_line_bytes: self.max_line_bytes.unwrap_or(default.max_line_bytes),
        }
    }
}

/// The [`ScanOptions`] one mapping pass runs under: the read flags'
/// [`ReadOptions`] and the already-resolved arrangement
/// ([`crate::parallel::Discovered::resolve`]), with no cancellation, which
/// `parse` adds.
///
/// **Resolved once per command and passed in, not re-resolved here.** `query`
/// passes its mapping pass's, its replay's going into [`pgdt_query::QueryOptions`]
/// ([`crate::parallel::Discovered::resolve_query`]).
pub(crate) fn scan_options(read: ReadFlags, parallel: &Resolved) -> ScanOptions {
    ScanOptions { read: read.options(), parallelism: parallel.parallelism(), cancel: None }
}

#[cfg(test)]
mod tests;
