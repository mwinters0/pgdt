//! `pgdt info`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Args;
use pgdt_dump::cache::{CacheStatus, Held, LoadedCache, OfflineCache, Opened};
use pgdt_io::Origin;

use crate::args::{IdentityArgs, cache_mode};
use crate::refusal::{refusal, spelled};
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
    // the library's sentence with `pgdt`'s spelling, then the `parse` the
    // ways out enable: it never writes. A strict-identity refusal is
    // about a source that moved, so it goes out through the classifier
    // the scanning commands use; the selection means here what it means
    // on the commands that scan (`docs/design/decisions.md`, "D21").
    // It reports the cache itself, never a `Dump` (`docs/design/decisions.md`, "D61").
    let Opened { cache, .. } = match mode.open(&origin).await {
        Ok(opened) => opened,
        Err(err @ pgdt_core::Error::CacheUnusable { .. }) => {
            anyhow::bail!(unusable_cache_message(err, Some(&origin)))
        }
        Err(e) => return Err(refusal(e, &origin)),
    };
    let loaded = match cache {
        Held::Loaded(loaded) => loaded,
        Held::Missing => {
            anyhow::bail!(
                "no cache at {} — run `pgdt parse --source {origin}` first",
                path.display()
            )
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
        CacheStatus::Missing => anyhow::bail!("no cache at {}", path.display()),
        CacheStatus::Unusable(unusable) => anyhow::bail!(unusable_cache_message(
            pgdt_core::Error::CacheUnusable { path: path.to_path_buf(), unusable },
            None
        )),
    }
}

/// What `pgdt info` says of a cache it cannot use: the library's own refusal,
/// [`pgdt_core::Error::CacheUnusable`], as the scanning commands say it —
/// worded once, so `info` and a scan never disagree on why — and, with a
/// source and a cache recognisably ours, the `parse` its ways out enable. A
/// file that is not a pgdt cache names no `parse`, no scan being allowed to
/// write over it (`docs/design/decisions.md`, "D20").
///
/// `source` is `None` in cache-only mode, which cannot reach
/// [`pgdt_core::Unusable::SourceChanged`] or
/// [`pgdt_core::Unusable::CompressionContradicted`] at
/// all, there being no live file to compare against — what its
/// `CacheOffline` diagnostic warns about.
fn unusable_cache_message(err: pgdt_core::Error, source: Option<&Origin>) -> String {
    let overwritable = matches!(
        &err,
        pgdt_core::Error::CacheUnusable { unusable, .. } if unusable.overwritable()
    );
    match source {
        Some(s) if overwritable => {
            format!("{}; `pgdt parse --source {s}` then builds a fresh one", refusal(err, s))
        }
        Some(s) => refusal(err, s).to_string(),
        None => spelled(&err),
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
