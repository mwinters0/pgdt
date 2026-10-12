//! The flag groups subcommands share, the CLI spelling of each library mode a
//! flag selects, and the value parsers of the subcommands' flags.

use clap::Args;
use pgdt_codec::{SchemaMode, UnrepresentableMode};
use pgdt_dump::ScanOptions;
use pgdt_dump::cache::StrictIdentity;
use pgdt_mapping::statistics::{StatisticsLevel, StatisticsSelection, StatisticsTarget};
use pgdt_model::PostgresInvalidValues;
use std::num::NonZeroU64;

use crate::parallel::Resolved;

/// CLI spelling of [`SchemaMode`] (`docs/design/decisions.md`, "D66").
#[derive(Clone, Copy, Default, clap::ValueEnum)]
pub(crate) enum CliSchemaMode {
    #[default]
    Typed,
    Strings,
}

impl From<CliSchemaMode> for SchemaMode {
    fn from(mode: CliSchemaMode) -> Self {
        match mode {
            CliSchemaMode::Typed => SchemaMode::Typed,
            CliSchemaMode::Strings => SchemaMode::Strings,
        }
    }
}

/// CLI spelling of [`UnrepresentableMode`] (`docs/design/decisions.md`,
/// "D98").
#[derive(Clone, Copy, Default, clap::ValueEnum)]
pub(crate) enum CliUnrepresentable {
    #[default]
    Null,
    Text,
    Refuse,
}

impl From<CliUnrepresentable> for UnrepresentableMode {
    fn from(mode: CliUnrepresentable) -> Self {
        match mode {
            CliUnrepresentable::Null => UnrepresentableMode::Null,
            CliUnrepresentable::Text => UnrepresentableMode::Text,
            CliUnrepresentable::Refuse => UnrepresentableMode::Refuse,
        }
    }
}

/// CLI spelling of [`PostgresInvalidValues`] (`docs/design/decisions.md`,
/// "D103").
#[derive(Clone, Copy, Default, clap::ValueEnum)]
pub(crate) enum CliInvalidValues {
    #[default]
    Default,
    Ignore,
}

impl From<CliInvalidValues> for PostgresInvalidValues {
    fn from(mode: CliInvalidValues) -> Self {
        match mode {
            CliInvalidValues::Default => PostgresInvalidValues::Default,
            CliInvalidValues::Ignore => PostgresInvalidValues::Ignore,
        }
    }
}

/// `parse`'s spelling of [`PostgresInvalidValues`], which adds the mode only a
/// parse has: `strict`, checking every field.
#[derive(Clone, Copy, Default, clap::ValueEnum)]
pub(crate) enum CliParseInvalidValues {
    #[default]
    Default,
    Ignore,
    Strict,
}

impl From<CliParseInvalidValues> for PostgresInvalidValues {
    fn from(mode: CliParseInvalidValues) -> Self {
        match mode {
            CliParseInvalidValues::Default => PostgresInvalidValues::Default,
            CliParseInvalidValues::Ignore => PostgresInvalidValues::Ignore,
            CliParseInvalidValues::Strict => PostgresInvalidValues::Strict,
        }
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

/// A `--statistics-level` value: a level, then comma-separated
/// `target=level` overrides, a target being a table or a
/// `schema.table.column`. A name is split at its dots, so a quoted identifier
/// holding one cannot be named here. **One target named twice is refused**,
/// whatever the two levels: the library reads the later of two equally
/// specific entries, and a person writing both meant one of them.
pub(crate) fn parse_statistics_level(
    text: &str,
) -> std::result::Result<StatisticsSelection, String> {
    fn level(word: &str) -> std::result::Result<StatisticsLevel, String> {
        match word.trim() {
            "data" => Ok(StatisticsLevel::Data),
            "metadata" => Ok(StatisticsLevel::Metadata),
            other => Err(format!("{other:?} is not a level: `data` or `metadata`")),
        }
    }
    let mut entries = text.split(',').map(str::trim);
    let first = entries.next().unwrap_or_default();
    if first.contains('=') {
        return Err(format!(
            "{first:?} is an override, and the first entry is the level of every table: start \
             with `data` or `metadata`"
        ));
    }
    let default = level(first)?;
    let mut overrides: Vec<(StatisticsTarget, StatisticsLevel)> = Vec::new();
    for entry in entries {
        let Some((name, stated)) = entry.split_once('=') else {
            return Err(format!(
                "{entry:?} states no level: write `{entry}=data` or `{entry}=metadata`"
            ));
        };
        let name = name.trim();
        let parts: Vec<&str> = name.split('.').collect();
        if parts.iter().any(|part| part.is_empty()) {
            return Err(format!("{name:?} is not a table or a schema.table.column"));
        }
        let target = match parts.as_slice() {
            [_] | [_, _] => StatisticsTarget::Table(name.to_string()),
            [schema, table, column] => StatisticsTarget::Column {
                table: format!("{schema}.{table}"),
                column: (*column).to_string(),
            },
            _ => return Err(format!("{name:?} has more parts than schema.table.column")),
        };
        if overrides.iter().any(|(named, _)| *named == target) {
            return Err(format!("{name:?} is named twice — give each table or column one level"));
        }
        overrides.push((target, level(stated)?));
    }
    Ok(StatisticsSelection { default, overrides })
}

/// A `--row-group-size` value: a byte count, never zero, which would
/// put every row in a group of its own past the end of the data, and a power
/// of two, so that every size a block's groups can merge to nests in it.
pub(crate) fn parse_row_group_size(text: &str) -> std::result::Result<NonZeroU64, String> {
    match text.parse::<u64>() {
        Ok(0) => Err("a group size of 0 covers no bytes".to_string()),
        Ok(n) if !n.is_power_of_two() => {
            Err(format!("a group size must be a power of two, and {n} is not"))
        }
        Ok(n) => Ok(NonZeroU64::new(n).expect("not zero")),
        Err(e) => Err(e.to_string()),
    }
}

/// The two read flags every scanning command carries, as given.
#[derive(Clone, Copy)]
pub(crate) struct ReadFlags {
    pub(crate) chunk_size: Option<usize>,
    pub(crate) max_line_bytes: Option<usize>,
}

/// The [`ScanOptions`] one scanning command runs under: the default, with
/// `--chunk-size` and `--max-line-bytes` applied where they were given and the
/// already-resolved arrangement ([`crate::parallel::Discovered::resolve`]).
///
/// **Resolved once per command and passed in, not re-resolved here.** `query`
/// passes its mapping pass's, its replay's going into [`pgdt_query::QueryOptions`]
/// ([`crate::parallel::Discovered::resolve_query`]).
pub(crate) fn scan_options(read: ReadFlags, parallel: &Resolved) -> ScanOptions {
    ScanOptions {
        chunk_size_bytes: read.chunk_size.unwrap_or(pgdt_io::SCAN_CHUNK_DEFAULT_SIZE_BYTES),
        max_line_bytes: read.max_line_bytes.unwrap_or(pgdt_dump::SCAN_LINE_DEFAULT_MAX_BYTES),
        parallelism: parallel.parallelism(),
        statistics_allowance_bytes: parallel.statistics_allowance(),
        ..ScanOptions::default()
    }
}

#[cfg(test)]
mod tests;
