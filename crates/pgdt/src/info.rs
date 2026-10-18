//! `pgdt info`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Args;
use pgdt_dump::cache::{
    CacheStatus, Held, LoadedCache, OVERWRITE_WAYS_OUT, OfflineCache, Opened, Unusable,
};
use pgdt_io::Origin;
use pgdt_model::CACHE_FORMAT_VERSION;

use crate::args::{IdentityArgs, cache_mode};
use crate::source::naming_the_source;
use json::print_index_json;
// `parse` prints the map it leaves as the listing prints it.
pub(crate) use text::{print_diagnostics, print_index, print_metadata};

mod json;
mod labels;
mod statistics;
mod text;

/// `pgdt info`'s flags.
#[derive(Args)]
pub(crate) struct InfoArgs {
    /// The dump the cache belongs to — a path or a URL, as `parse`
    /// takes it: its size is checked against the cache's, so a changed
    /// source is caught. Omit it to answer from `--dtcache` alone —
    /// cache-only mode, which then requires `--dtcache` and cannot check
    /// anything.
    #[arg(long)]
    source: Option<String>,
    /// Cache file path, defaulting to the dump's name plus `.dtcache`:
    /// beside a local dump, and for a URL in the working directory, named
    /// after the URL's last path segment. Required when `--source` is
    /// omitted — that is the cache-only entry point, and there is nothing
    /// else to answer from.
    #[arg(long, required_unless_present = "source")]
    dtcache: Option<PathBuf>,
    /// Also report each `COPY` block's byte offsets and, per column that
    /// has something to say, what it became: the Arrow type it resolved
    /// to, or — for a column that came back as a string for a reason —
    /// why. A column that is simply text says nothing. Names beneath
    /// each block what a strict parse does not check in it, which the
    /// listing otherwise counts. Turns the `user-defined types` count
    /// into a listing of the types themselves, on a compressed dump adds
    /// the container's shape, and closes the listing, above its totals,
    /// with the statistics `parse` gathered, per table and column: over
    /// how many blocks, at what group size, and in how many groups each
    /// column keeps bounds and a dictionary.
    #[arg(long)]
    detail: bool,
    /// List every span the map holds (`docs/design/decisions.md`,
    /// "D34") — DDL objects
    /// and framing included, not just `COPY` blocks — instead of the
    /// per-table listing.
    #[arg(long)]
    map: bool,
    /// Print the internal index as compact JSON instead of the
    /// human-readable listing: the whole cache file — the `DumpIndex`,
    /// every block's per-group statistics included, and the envelope
    /// around it: format version, container kind, seek table and source
    /// identity — with its coverage, its diagnostics, and the
    /// per-`COPY`-block type resolution `--detail` renders as text.
    /// Statistics are not rolled up per table, as `--detail` rolls them.
    /// No schema stability is promised — this is a raw dump of our
    /// internal representation, not a supported interchange format
    /// (`docs/design/decisions.md`, "D67"). Incompatible with
    /// `--detail`/`--map`, which format detail this already carries in
    /// full.
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    identity: IdentityArgs,
}

/// `pgdt info`: report what the cache holds, never scanning.
pub(crate) async fn run(args: InfoArgs) -> Result<()> {
    let InfoArgs { source: file, dtcache, detail, map, json, identity } = args;
    if json && (detail || map) {
        anyhow::bail!(
            "--json already carries everything --detail/--map would add — drop one of them"
        );
    }
    let Some(file) = file else {
        // Cache-only mode (`docs/design/decisions.md`, "D61"): no
        // live dump file at all, so clap already required
        // `--dtcache` for us — and refused `--strict-identity`, which
        // has nothing here to bind.
        let path = dtcache.expect("clap requires --dtcache when --source is omitted");
        return info_offline(&path, detail, map, json).await;
    };
    // `info` never scans, so `--dtcache none` — "ignore the cache" —
    // leaves nothing to answer from, and the message names the way
    // out. Formed here rather than in `Error::CacheDisabled` because
    // it interpolates the user's own `--source` path, which the
    // library error does not have.
    let origin = Origin::resolve(&file)?;
    // `info` reads nothing of the dump past opening it, so a source no
    // in-flight check can cover is never refused here
    // (`SourceWatch::require_checkable` is where a read begins).
    let mode = cache_mode(&origin, dtcache.as_deref()).with_strict_identity(identity.resolve());
    let path = mode
        .require_enabled("info")
        .with_context(|| {
            format!(
                "`--dtcache none` cannot be combined with `info`, which never scans — run \
                 `pgdt parse --source {origin} --dtcache <path>` to build a cache \
                 somewhere writable, then pass that same `--dtcache <path>` here"
            )
        })?
        .to_path_buf();
    // A cache `info` cannot use leaves it nothing to report from, and
    // it says so having read no more than the file's leading bytes, in
    // its own sentence: it never writes, so what it names is the
    // `parse` the ways out enable. A strict-identity refusal is about a
    // source that moved, so it goes out through the classifier the
    // scanning commands use; the selection means here what it means on
    // the commands that scan (`docs/design/decisions.md`, "D21").
    let Opened { cache, .. } = match mode.open(&origin).await {
        Ok(opened) => opened,
        Err(pgdt_core::Error::CacheUnusable { unusable, .. }) => {
            anyhow::bail!(unusable_cache_message(
                &CacheStatus::Unusable(unusable),
                &path,
                Some(&origin)
            ))
        }
        Err(e) => return Err(naming_the_source(e, &origin)),
    };
    let loaded = match cache {
        Held::Loaded(loaded) => loaded,
        Held::Missing => {
            anyhow::bail!(unusable_cache_message(&CacheStatus::Missing, &path, Some(&origin)))
        }
        Held::Disabled | Held::Replacing(_) => {
            unreachable!("`info` opens an enabled cache it never replaces")
        }
    };
    report(&loaded, detail, map, json)
}

/// `pgdt info` with no `--source`: answer strictly from the cache at `path`
/// (`docs/design/decisions.md`, "D61"). An incomplete cache is
/// reported like any other, with its coverage stated — cache-only mode has no
/// scan to extend it with, but "as far as the scan got" is still an answer.
async fn info_offline(path: &Path, detail: bool, map: bool, json: bool) -> Result<()> {
    match OfflineCache::new(path).load().await? {
        CacheStatus::Loaded(loaded) => report(&loaded, detail, map, json),
        unusable => anyhow::bail!(unusable_cache_message(&unusable, path, None)),
    }
}

/// The sentence `pgdt info` prints for a cache it cannot use — one per cause,
/// because the fact the user needs to know differs. Each ends as the scanning
/// commands' refusals do, in `OVERWRITE_WAYS_OUT`, and then names the `parse`
/// those ways out enable; a file that is not a pgdt cache names neither, no
/// scan being allowed to write over it (`docs/design/decisions.md`, "D20"),
/// and a missing one names only the `parse` that builds it.
///
/// **One match, two renderings**, so a new [`Unusable`] variant has to answer
/// both. `source` is `None` in cache-only mode, which states the fault and
/// stops; it cannot reach [`Unusable::SourceChanged`] or
/// [`Unusable::CompressionContradicted`] at all, there being no live file to
/// compare against — what its `CacheOffline` diagnostic warns about.
///
/// Takes the whole [`CacheStatus`] rather than a narrowed type so the match
/// stays exhaustive: a usable status reaching here is a caller bug.
fn unusable_cache_message(status: &CacheStatus, path: &Path, source: Option<&Origin>) -> String {
    let shown = path.display();
    let live = || source.expect("cache-only mode has no live source to compare against");
    let found = match status {
        CacheStatus::Missing => {
            return match source {
                Some(s) => format!("no cache at {shown} — run `pgdt parse --source {s}` first"),
                None => format!("no cache at {shown}"),
            };
        }
        CacheStatus::Unusable(Unusable::NotACache) => {
            return format!("{shown} is not a pgdt cache — check the path");
        }
        CacheStatus::Unusable(Unusable::Unreadable) => {
            format!("the cache at {shown} is cut short or damaged and cannot be read")
        }
        CacheStatus::Unusable(Unusable::UnsupportedVersion { found }) => format!(
            "the cache at {shown} was written by a different pgdt build (cache format {found}, \
             where this build reads {CACHE_FORMAT_VERSION}) and cannot be read"
        ),
        CacheStatus::Unusable(Unusable::SourceChanged {
            cached_stored_size: cached_size,
            live_stored_size: live_size,
        }) => format!(
            "{} has changed since it was parsed ({live_size} bytes now, {cached_size} when the \
             cache at {shown} was written), so every offset in the cache could be wrong",
            live()
        ),
        CacheStatus::Unusable(Unusable::CompressionContradicted) => format!(
            "the cache at {shown} records compression details that {} contradicts, so it was \
             written for another file",
            live()
        ),
        CacheStatus::Unusable(Unusable::CalendarChanged { counted_under, build }) => format!(
            "the cache at {shown} counted the values a query cannot display against a calendar \
             ending {}, where this build's ends {}, so it cannot be read",
            pgdt_codec::decode::render_date32(*counted_under),
            pgdt_codec::decode::render_date32(*build),
        ),
        CacheStatus::Loaded(_) => unreachable!("a usable cache is reported, not refused"),
    };
    match source {
        Some(s) => format!(
            "{found}{OVERWRITE_WAYS_OUT}; `pgdt parse --source {s}` then builds a fresh one"
        ),
        None => found,
    }
}

/// How much of the file the index covers, stated **once, at the top**, with
/// nothing below it qualified (`docs/design/decisions.md`, "D67").
///
/// A partial index lacks *records*, not confidence: a block enters the map
/// only at a `CopyEnd` watermark, holding what its level records of every
/// row, so every record it holds is complete in itself, and there is no
/// half-known block.
/// The percentage floors, so it reads 100% only for a finished scan.
fn completion_line(scanned_through: u64, total_size: u64) -> String {
    // A zero-byte file is trivially covered in full, and has no ratio.
    let percent = (scanned_through.min(total_size) * 100).checked_div(total_size).unwrap_or(100);
    format!("Scan completion: {percent}% ({scanned_through} bytes)")
}

/// Every `pgdt info` rendering goes through here: the coverage line, then the
/// listing — or the export alone, which carries its coverage as fields.
fn report(loaded: &LoadedCache, detail: bool, map: bool, json: bool) -> Result<()> {
    let LoadedCache { index, total_size, compression, envelope, .. } = loaded;
    let complete = loaded.is_complete();
    if json {
        return print_index_json(index, *total_size, *compression, envelope, complete);
    }
    println!("{}", completion_line(index.scanned_through, *total_size));
    println!();
    print_index(index, *compression, detail, map, complete, detail);
    Ok(())
}
