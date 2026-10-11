//! `pgdt info`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Args;
use pgdt_dump::cache::{CacheEnvelope, CacheMode, CacheStatus, CompressionShape};
use pgdt_io::Origin;
use pgdt_model::DumpIndex;

use crate::args::IdentityArgs;
use crate::source::{Opened, naming_the_source, open_with_cache, unusable_cache_message};
use json::print_index_json;
use text::print_index;

mod json;
mod labels;
mod statistics;
pub(crate) mod text;

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
    let mode =
        CacheMode::resolve(&origin, dtcache.as_deref()).with_strict_identity(identity.resolve());
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
    // `parse` the ways out enable.
    let source = match open_with_cache(&origin, &mode).await? {
        Opened::Source { source, .. } => source,
        Opened::Unusable(why) => {
            anyhow::bail!(unusable_cache_message(&CacheStatus::Unusable(why), &path, Some(&origin)))
        }
    };
    let status = pgdt_dump::cache::load(&path, source.as_ref()).await?;
    let (mut index, weak, cache_origin, total_size, compression, envelope) = match status {
        CacheStatus::Valid { index, weak, origin, total_size, compression, envelope }
        | CacheStatus::Incomplete { index, weak, origin, total_size, compression, envelope } => {
            (index, weak, origin, total_size, compression, envelope)
        }
        unusable => {
            anyhow::bail!(unusable_cache_message(&unusable, &path, Some(&origin)))
        }
    };
    // `info` reads the whole status rather than going through
    // `CacheMode::load`, so it asks for that method's check by name;
    // the selection means here what it means on the commands that
    // scan (`docs/design/decisions.md`, "D21"). One meaning is one
    // rendering too: the refusal is about a source that moved, so it
    // goes out through the classifier the scanning commands use
    // rather than straight into `anyhow`.
    if let Some(refusal) = mode.strict_identity_refusal(&weak, &cache_origin) {
        return Err(naming_the_source(refusal, &origin));
    }
    // Reported rather than acted on: between runs the weak signals are
    // advisory unless a selection binds them
    // (`docs/design/decisions.md`, "D21";
    // `docs/design/decisions.md`, "D87"). Asked for by
    // name, for the same reason the refusal above is.
    index.diagnostics.extend(pgdt_dump::cache::advisory_identity_diagnostics(&weak, &cache_origin));
    report(&index, total_size, compression, &envelope, detail, map, json)?;
    Ok(())
}

/// `pgdt info` with no `--source`: answer strictly from the cache at `path`
/// (`docs/design/decisions.md`, "D61"). An `Incomplete` cache is
/// reported like any other, with its coverage stated — cache-only mode has no
/// scan to extend it with, but "as far as the scan got" is still an answer.
async fn info_offline(path: &Path, detail: bool, map: bool, json: bool) -> Result<()> {
    let mode = CacheMode::Offline(path.to_path_buf());
    let (index, total_size, compression, envelope) = match mode.load_offline().await? {
        CacheStatus::Valid { index, total_size, compression, envelope, .. }
        | CacheStatus::Incomplete { index, total_size, compression, envelope, .. } => {
            (index, total_size, compression, envelope)
        }
        unusable => anyhow::bail!(unusable_cache_message(&unusable, path, None)),
    };
    report(&index, total_size, compression, &envelope, detail, map, json)
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
fn report(
    index: &DumpIndex,
    total_size: u64,
    compression: Option<CompressionShape>,
    envelope: &CacheEnvelope,
    detail: bool,
    map: bool,
    json: bool,
) -> Result<()> {
    let complete = index.is_complete(total_size);
    if json {
        return print_index_json(index, total_size, compression, envelope, complete);
    }
    println!("{}", completion_line(index.scanned_through, total_size));
    println!();
    print_index(index, compression, detail, map, complete, detail);
    Ok(())
}
