//! `pgdt parse`.

use std::num::NonZeroU64;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Args;
use namespace_init::InitShutdown;
use pgdt_dump::cache::Held;
use pgdt_dump::preamble_only;
use pgdt_io::Origin;
use pgdt_mapping::statistics::{
    ROW_GROUP_DEFAULT_MIN_ROWS, StatisticsLevel, StatisticsRequest, StatisticsSelection,
    StatisticsSizing, StatisticsTarget,
};
use pgdt_model::PostgresInvalidValues;
use tokio_util::sync::CancellationToken;

use crate::args::{
    IdentityArgs, ModeParser, OverwriteArgs, ReadFlags, cache_mode, parse_chunk_size,
    parse_max_line_bytes, scan_options,
};
use crate::info::{print_diagnostics, print_index, print_metadata};
use crate::interrupt::install_interrupt_guard;
use crate::introspect;
use crate::parallel::ParallelArgs;
use crate::source::naming_the_source;

/// `pgdt parse`'s flags.
#[derive(Args)]
pub(crate) struct ParseArgs {
    /// The dump to scan: a path, or an `http://` or `https://` URL a
    /// server answers ranged GETs for. A `file://` URL is a path on this
    /// machine. Any other scheme is refused by name, as is a URL carrying
    /// a username or password — no credential is ever sent, and a
    /// presigned URL needs none.
    #[arg(long)]
    source: String,
    /// Cache file path, or `none` to disable the cache. Since `parse`'s
    /// whole purpose is to write the cache, `none` is rejected.
    #[arg(long)]
    dtcache: Option<PathBuf>,
    /// Scan only the dump's preamble — the dump-level header alone, no
    /// per-block listing or row counts — instead of the whole file. Cost
    /// is independent of dump size regardless of how much `COPY` data
    /// follows (`docs/design/decisions.md`, "D30"). The cache this leaves is a partial one that `pgdt info`
    /// reads like any other.
    #[arg(long)]
    preamble_only: bool,
    /// Bytes requested per read from the dump. The default, 1 MiB, is
    /// the fastest of the six sizes measured on the one device class
    /// where a chunk size shows anything at all, and makes no measurable
    /// difference on the other two (`docs/design/measurements.md`, "What the read
    /// chunk size is worth") — so this is a tuning escape hatch for a
    /// device unlike those, not a knob with a win behind it. A raised
    /// value keeps its buffer pooling and costs memory instead: the read
    /// path holds four buffers of whatever size you ask for, or what
    /// `--memory` leaves for read buffers, whichever is fewer.
    #[arg(long, value_name = "BYTES", value_parser = parse_chunk_size)]
    chunk_size: Option<usize>,
    /// The longest line the scan accepts, in bytes. A longer one stops the
    /// scan with an error naming its offset rather than being buffered
    /// without bound; the check is made as each read completes, so a line
    /// can overrun the limit by up to one read chunk before it is refused.
    /// The default, 64 MiB, holds any row of ordinary
    /// values; a dump holding larger ones — a single value of hundreds of
    /// megabytes — needs a larger limit stated here. A row is held whole
    /// while it is scanned, so this is also what one row may cost in
    /// memory.
    #[arg(long, value_name = "BYTES", value_parser = parse_max_line_bytes)]
    max_line_bytes: Option<usize>,
    /// How much to record of each table: a level for every table, `data`
    /// (the default) or `metadata`, then comma-separated overrides naming
    /// a table (`schema.table`, or a bare `table` in any schema) or a
    /// column (`schema.table.column`) and its own level. So
    /// `metadata,public.foo=data` records every table at the metadata
    /// level but `public.foo`, and `data,public.foo.blob=metadata` every
    /// column of every table at the data level but `blob`. The most
    /// specific entry naming a column decides its level — a column, then
    /// a qualified table, then a bare one, then the first entry — and one
    /// table or column named twice is refused.
    ///
    /// The data level is what queries are for. A table any of whose
    /// columns is at it has its array columns' shapes recorded, which a
    /// typed query needs, and each such column its statistics: for each
    /// stretch of the table's data, its row count and the column's NULL
    /// count, and where the column's comparison allows, its least and
    /// greatest value and its distinct values. Gathering reads every value
    /// of every such column, which costs the scan time and memory.
    ///
    /// The metadata level records where a table's data lies and how many
    /// rows it holds, and nothing drawn from those rows, so it scans as
    /// fast as the file allows. A `query` of such a table reads its rows
    /// once more first, every time, and the DataFusion provider refuses
    /// it. A block an earlier `parse` recorded at a lower level than asked
    /// here is re-read once the rest of the file is scanned, keeping
    /// whatever it already had: nothing recorded is ever dropped.
    #[arg(
        long,
        value_name = "LEVELS",
        value_parser = parse_statistics_level,
        conflicts_with = "preamble_only"
    )]
    statistics_level: Option<StatisticsSelection>,
    /// The bytes of a table's data each row group of statistics covers, a
    /// power of two. The default, 1 MiB, is coarse, and doubles for a
    /// table whose data would take more than 4,096 groups until it takes
    /// no more, and for a table whose rows are too wide for its groups to
    /// hold `--row-group-min-rows`; it halves instead for one too dense
    /// for a stated `--row-group-max-rows`, which lifts both of those; a
    /// stated size is kept exactly, and a
    /// smaller group records more finely where values lie and costs memory
    /// and cache space in proportion. Stated, it also re-reads every block
    /// gathered at another size; left unstated, a block keeps the size it
    /// was gathered at.
    #[arg(
        long,
        value_name = "BYTES",
        value_parser = parse_row_group_size,
        conflicts_with = "preamble_only"
    )]
    row_group_size: Option<NonZeroU64>,
    /// The fewest rows a row group of statistics should hold under the
    /// default group size. Once a table's data is read, its groups double
    /// until at most half of them fall short of this many rows or the
    /// table is one group, so a table of wide rows keeps fewer groups; the default,
    /// 1,024, leaves rows up to about 1 KiB wide at 1 MiB a group, and 0
    /// doubles nothing. Stated, it also re-reads every block sized under
    /// another minimum or at a stated group size; left unstated, a block
    /// keeps the size it was gathered at. A stated
    /// `--row-group-size` is kept exactly, so the two are refused
    /// together.
    #[arg(
        long,
        value_name = "ROWS",
        conflicts_with_all = ["preamble_only", "row_group_size"]
    )]
    row_group_min_rows: Option<u64>,
    /// The most rows a row group of statistics should hold under the
    /// default group size, read at the 90th-percentile group so that at
    /// most a tenth of them hold more. There is no default: stated, it
    /// stops the doubling `--row-group-min-rows` would otherwise do, it
    /// lifts the 4,096-group ceiling, and a table too dense to meet it at
    /// 1 MiB a group is read a second time, at the finer size its groups
    /// predict — once, keeping what that gives and saying so if it still
    /// misses. Stated, it also re-reads every block sized under another
    /// maximum or at a stated group size; left unstated, a block keeps the
    /// size it was gathered at. Refused below `--row-group-min-rows`,
    /// which defaults to 1,024, and beside a stated
    /// `--row-group-size`, which is kept exactly.
    #[arg(
        long,
        value_name = "ROWS",
        conflicts_with_all = ["preamble_only", "row_group_size"]
    )]
    row_group_max_rows: Option<u64>,
    /// What to do with a value PostgreSQL refuses for its column's type —
    /// `70000` in a `smallint`, a `double precision` written past its
    /// range: `default` stops the scan at the first one it reads, as a
    /// restore of the dump stops there, or at one an `ignore` parse
    /// recorded in the cache; `ignore` goes on past each, keeping no
    /// bounds, sum or dictionary of the stretch of its column it sits in
    /// and recording
    /// it, and a `query` then reads it as its own
    /// `--postgres-invalid-values` says; `strict` stops as `default` does
    /// but reads every value to find one, those `default` leaves to a
    /// query included, re-reading each table the cache holds that no
    /// `strict` parse checked, so a clean run means every value of a type
    /// pgdt reads was checked; the listing names, under each table, what
    /// `strict` does not check.
    #[arg(
        long,
        value_name = "MODE",
        default_value_t,
        value_parser = ModeParser::every(&PostgresInvalidValues::ALL),
        conflicts_with = "preamble_only"
    )]
    postgres_invalid_values: PostgresInvalidValues,
    #[command(flatten)]
    identity: IdentityArgs,
    #[command(flatten)]
    overwrite: OverwriteArgs,
    #[command(flatten)]
    parallel: ParallelArgs,
}

/// `pgdt parse`: scan, save, and list what the cache now holds.
pub(crate) async fn run(args: ParseArgs, init: &mut InitShutdown) -> Result<()> {
    let ParseArgs {
        source: file,
        dtcache,
        preamble_only: preamble_only_flag,
        chunk_size,
        max_line_bytes,
        statistics_level,
        row_group_size,
        row_group_min_rows,
        row_group_max_rows,
        postgres_invalid_values,
        identity,
        overwrite,
        parallel,
    } = args;
    let read = ReadFlags { chunk_size, max_line_bytes };
    let statistics = statistics_request(
        statistics_level,
        row_group_size,
        row_group_min_rows,
        row_group_max_rows,
    )?;
    // `parse` scans to persist (`docs/design/decisions.md`, "D61").
    // Reject `--dtcache none` up front, before paying for a scan we
    // won't be allowed to persist.
    let origin = Origin::resolve(&file)?;
    let mode = cache_mode(&origin, dtcache.as_deref())
        .with_strict_identity(identity.resolve())
        .with_overwrite_unusable(overwrite.overwrite_unusable_cache);
    let path = mode
        .require_enabled("parse")
        .context("`--dtcache none` cannot be combined with `parse`")?
        .to_path_buf();
    // The dump is opened through its cache once, the cache decoded once
    // and handed to the pass: one that cannot be used is refused here as
    // it is by `info` and `query`, having read no more than the file's
    // leading bytes, unless `--overwrite-unusable-cache` says it may be
    // replaced (`docs/design/decisions.md`, "D20"). The flags and the
    // limit are announced ahead of it, neither waiting on the file (D64).
    let stated = parallel.discover();
    stated.announce();
    let opened = mode.open(&origin).await.map_err(|e| naming_the_source(e, &origin))?;
    let source = Arc::clone(&opened.source);
    let cached_statistics = match &opened.cache {
        Held::Loaded(loaded) => loaded.index.statistics_heap_bytes(),
        Held::Missing | Held::Disabled | Held::Replacing(_) => 0,
    };
    let parallel = stated.resolve(source.as_ref(), cached_statistics);
    parallel.announce();
    if preamble_only_flag {
        let (metadata, diagnostics) = preamble_only(opened, &scan_options(read, &parallel))
            .await
            .map_err(|e| naming_the_source(e, &origin))?;
        print_metadata(&metadata, false);
        print_diagnostics(&diagnostics);
        println!();
        println!("wrote cache to {}", path.display());
        return Ok(());
    }
    let size = source.size().await?;
    let cancel = CancellationToken::new();
    let guard = install_interrupt_guard(cancel.clone(), init)?;
    let run = pgdt_mapping::map_file(
        opened,
        read.options(),
        parallel.arrangement(),
        postgres_invalid_values,
        &statistics,
        &cancel,
    )
    .await
    .map_err(|e| naming_the_source(e, &origin))?;
    introspect::statistics_returned(&run.statistics);
    if run.interrupted {
        // No listing: `pgdt info` is the command that reports. Both
        // lines go to stderr, so a caller redirecting stdout gets an
        // empty report rather than a truncated one.
        // A stop inside the statistics back-fill leaves the map
        // whole, and only the same statistics flags pick it up again.
        if run.index.is_complete(size) && run.lacking_statistics == 0 {
            eprintln!(
                "interrupted after checking {} block(s) no strict parse had checked — the \
                 cache at {} holds those checked so far",
                run.checked,
                path.display()
            );
            eprintln!("re-run `pgdt parse --source {origin}` with the same flags to continue");
        } else if run.index.is_complete(size) {
            eprintln!(
                "interrupted after re-reading {} of the {} block(s) lacking the requested \
                 statistics — the cache at {} holds those re-read so far",
                run.backfilled,
                run.lacking_statistics,
                path.display()
            );
            eprintln!(
                "re-run `pgdt parse --source {origin}` with the same statistics flags to \
                 continue"
            );
        } else if run.index.scanned_through == 0 {
            // Nothing was banked, so there is no file to name and
            // nothing to continue from: the interrupt landed before
            // the first watermark, and the preamble prepass holds its
            // spans aside until they are whole, so no cache was
            // written (`docs/design/decisions.md`, "D26"). Naming one
            // here would send a reader to a file that may not exist,
            // and "continue" would promise a resume from byte 0.
            eprintln!("interrupted at byte 0 of {size} — nothing was scanned or written");
            eprintln!("re-run `pgdt parse --source {origin}` to scan from the start");
        } else {
            eprintln!(
                "interrupted at byte {} of {size} — the cache at {} holds the scan so far",
                run.index.scanned_through,
                path.display()
            );
            eprintln!("re-run `pgdt parse --source {origin}` to continue");
        }
        guard.die_interrupted();
    }
    // The run completed and its cache is saved: from here a signal
    // ends the process in its handler, and one that landed past the
    // mapping pass's last check ends it now, ahead of a listing it
    // would cut short (`docs/design/decisions.md`, "D26").
    guard.arm();
    // The listing describes the file's state after this run, not
    // this invocation's diff, so the one line that *is* about the
    // invocation goes above it.
    if let Some(notice) = resume_notice(run.resumed_from, size, run.backfilled, run.checked) {
        println!("{notice}");
        println!();
    }
    // `map_file` reached EOF, so its censuses cover the whole file.
    // `--detail` is off, so the container line is not printed and
    // nothing has to be read for it; a strict parse names what it
    // left unchecked, any other gives the count alone.
    let strict = postgres_invalid_values == PostgresInvalidValues::Strict;
    print_index(&run.index, None, false, false, true, strict);
    println!();
    println!("wrote cache to {}", path.display());
    Ok(())
}

/// What `parse` records, from its four statistics flags: `--statistics-level`
/// absent is the data level everywhere, a size or a bound beside a level
/// gathering nothing is refused rather than ignored, and so is a maximum below
/// the minimum in force — the stated one, or the default where none was
/// stated, which is the minimum the run would apply.
fn statistics_request(
    selection: Option<StatisticsSelection>,
    group_size: Option<NonZeroU64>,
    min_rows: Option<u64>,
    max_rows: Option<u64>,
) -> Result<StatisticsRequest> {
    // `clap` refuses a bound beside a size, so a stated size has none to drop.
    let sizing = match group_size {
        Some(size) => StatisticsSizing::Stated(size),
        None => StatisticsSizing::Density { min_rows, max_rows },
    };
    let request = StatisticsRequest { selection: selection.unwrap_or_default(), sizing };
    if !request.gathers() {
        let sizing = [
            (group_size.is_some(), "--row-group-size"),
            (min_rows.is_some(), "--row-group-min-rows"),
            (max_rows.is_some(), "--row-group-max-rows"),
        ];
        if let Some((_, flag)) = sizing.iter().find(|(stated, _)| *stated) {
            anyhow::bail!(
                "{flag} sizes statistics, and `--statistics-level` puts every table at the \
                 metadata level, which gathers none — drop one of them"
            );
        }
    }
    let floor = min_rows.unwrap_or(ROW_GROUP_DEFAULT_MIN_ROWS);
    if max_rows.is_some_and(|max_rows| max_rows < floor) {
        let stated = if min_rows.is_some() { "" } else { " by default" };
        anyhow::bail!(
            "--row-group-max-rows {} is below the {floor} rows \
             --row-group-min-rows asks for{stated} — no group size holds both",
            max_rows.expect("read above")
        );
    }
    Ok(request)
}

/// The one line `pgdt parse` prints about *this invocation* rather than about
/// the file: where the scan picked up. `None` for a scan that started at byte
/// 0.
///
/// A run that found the cache already complete scanned nothing at all and
/// says so, rather than reporting a resume point equal to the file's size —
/// two different facts to a user checking whether a scan finished — unless it
/// re-read blocks for statistics they lacked (`backfilled`), or to check them
/// under `--postgres-invalid-values strict` (`checked`), which stderr counts.
fn resume_notice(
    resumed_from: u64,
    size: u64,
    backfilled: usize,
    checked: usize,
) -> Option<String> {
    let reread = match (backfilled, checked) {
        (0, 0) => None,
        (_, 0) => Some("blocks lacking the requested statistics"),
        (0, _) => Some("blocks no strict parse had checked"),
        _ => Some("blocks lacking the requested statistics or that no strict parse had checked"),
    };
    match resumed_from {
        0 => None,
        n if n >= size && reread.is_some() => Some(format!(
            "the cache already covers all {size} byte(s); only {} were re-read",
            reread.expect("just tested")
        )),
        n if n >= size => {
            Some(format!("nothing to scan: the cache already covers all {size} byte(s)"))
        }
        n => Some(format!("resumed a previous scan at byte {n} of {size}")),
    }
}

/// A `--statistics-level` value: a level, then comma-separated
/// `target=level` overrides, a target being a table or a
/// `schema.table.column`. A name is split at its dots, so a quoted identifier
/// holding one cannot be named here. **One target named twice is refused**,
/// whatever the two levels: the library reads the later of two equally
/// specific entries, and a person writing both meant one of them.
fn parse_statistics_level(text: &str) -> std::result::Result<StatisticsSelection, String> {
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
fn parse_row_group_size(text: &str) -> std::result::Result<NonZeroU64, String> {
    match text.parse::<u64>() {
        Ok(0) => Err("a group size of 0 covers no bytes".to_string()),
        Ok(n) if !n.is_power_of_two() => {
            Err(format!("a group size must be a power of two, and {n} is not"))
        }
        Ok(n) => Ok(NonZeroU64::new(n).expect("not zero")),
        Err(e) => Err(e.to_string()),
    }
}
