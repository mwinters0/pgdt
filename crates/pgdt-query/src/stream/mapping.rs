//! Pass 1 of a query, whole: the request checked, the map loaded or
//! extended until the queried table is settled, and the census a
//! metadata-level table lacks.

use std::sync::Arc;

use pgdt_codec::SchemaMode;
#[cfg(feature = "introspect")]
use pgdt_core::internal::statistics_loaded;
use pgdt_core::{Error, Result};
use pgdt_dump::ScanOptions;
use pgdt_dump::attach_text;
use pgdt_dump::cache::{CacheLoad, CacheMode, SourceWatch};
use pgdt_dump::internal::scan_preamble;
use pgdt_io::ByteRangeSource;
use pgdt_model::{CopyBlock, DumpIndex, DumpMetadata};

use super::resume::ResumeToken;
use crate::options::{QueryOptions, ScanExtent};
use pgdt_mapping::internal::{MapStop, Pass, announce_read_loop, map_forward, reread_rows, splice};

/// What a query's mapping pass settled: the blocks this query will replay,
/// and the DDL to type them against.
pub(super) struct MappedTable {
    pub(super) matches: Vec<CopyBlock>,
    pub(super) metadata: Option<DumpMetadata>,
}

/// The two checks that are about the *request* rather than about the file, so
/// they fire before a byte is read and whether or not the table turns up: a
/// duplicate projection column, and a resume token from another query.
pub(super) fn validate_request(
    query_options: &QueryOptions,
    resume: Option<&ResumeToken>,
    fingerprint: u64,
) -> Result<()> {
    if let Some(columns) = query_options.projection.as_deref() {
        for (i, name) in columns.iter().enumerate() {
            if columns[..i].contains(name) {
                return Err(Error::DuplicateProjectionColumn { column: name.clone() });
            }
        }
    }
    if resume.is_some_and(|t| t.query_fingerprint != fingerprint) {
        return Err(Error::ResumeQueryMismatch);
    }
    Ok(())
}

/// Pass 1 of a query, whole: load or start the map, capture the preamble,
/// extend the map until this query's table is settled, then narrow the
/// name-only matches to at most one candidate, whose census the plan takes
/// ([`TableColumns`](super::replay::plan::TableColumns)). Yields
/// no rows — see the module docs. **Both entry points run exactly this**, so a
/// partitioned replay maps the file once rather than once per sub-stream.
pub(super) async fn map_for_query(
    source: &dyn ByteRangeSource,
    table: &str,
    scan_options: &ScanOptions,
    query_options: &QueryOptions,
    cache: &CacheMode,
    watch: &SourceWatch,
) -> Result<MappedTable> {
    let size = source.size().await?;

    let mut index = match cache.load(source).await? {
        CacheLoad::Index(index) => index,
        // Nothing to build forward from, and nothing at the path to keep.
        CacheLoad::Disabled | CacheLoad::Missing => DumpIndex::default(),
        // Refused before the dump is read past its magic, unless the caller said
        // this cache may be replaced (`docs/design/decisions.md`, "D20").
        CacheLoad::Unusable(unusable) => match cache.refusal(&unusable) {
            Some(refusal) => return Err(refusal),
            None => DumpIndex::default(),
        },
    };
    // The one statistics term a query has: it keeps no account, so nothing else
    // says what the cache handed it.
    #[cfg(feature = "introspect")]
    statistics_loaded(index.statistics_heap_bytes());

    // The first database's preamble is captured before anything else runs,
    // whatever table this call queries (`docs/design/decisions.md`, "D30").
    // `CacheMode::Disabled` still runs the scan, with `cache.save` a no-op.
    // Persisted immediately, so it survives a caller that polls the stream
    // once and drops it.
    let first_db_preamble_known = index
        .metadata
        .as_ref()
        .and_then(|m| m.databases.first())
        .is_some_and(|db| db.preamble_complete);
    if !first_db_preamble_known {
        // The prepass's spans are kept, not discarded: they tile
        // `[0, preamble_end)`, which is the prefix `map_forward` splices its
        // own output onto.
        let (metadata, spans, preamble_end, roles, tablespaces) =
            scan_preamble(source, scan_options).await?;
        index.metadata = Some(metadata);
        index.spans = splice(&[], spans, 0, preamble_end, size);
        index.roles.extend(roles);
        index.tablespaces.extend(tablespaces);
        index.scanned_through = index.scanned_through.max(preamble_end);
        attach_text(source, &mut index.spans).await?;
        cache.save(watch, source, &index).await?;
    }
    // Pass 1: extend the map until this query's table is settled. No
    // rows come out of this, and nothing is yielded until it returns.
    let selector = query_options.database.as_deref();
    let target = match query_options.scan_extent {
        ScanExtent::UntilTargetSettled => Some((table, selector)),
        ScanExtent::Full => None,
    };
    // A cancelled mapping pass is an error here rather than a short stream
    // (`docs/design/decisions.md`, "D48"). `pgdt query` never sets the flag;
    // an embedder that does gets told.
    // A pass gathering nothing charges nothing, so its account is never read.
    let account = Arc::default();
    if map_forward(
        source,
        scan_options,
        cache,
        watch,
        &mut index,
        target,
        Pass::Query,
        &account,
        size,
    )
    .await?
        == MapStop::Interrupted
    {
        return Err(Error::ScanCancelled { scanned_through: index.scanned_through });
    }

    // Read *after* the mapping pass, not before it: `map_forward` states the
    // metadata at each `\connect`ed database's first `COPY` block, so the
    // schema depends on the map and never on how far the row replay has got.
    let metadata = index.metadata.clone();

    // One target per query (`docs/design/decisions.md`, "D49"): narrow the
    // name-only matches down to at most one `(database, qualified name)`
    // candidate before reading any of them, so a would-be silent union across
    // schemas or databases errors instead. `query_options.database`, when
    // given, filters candidates first rather than picking among them after
    // the fact. An ambiguous name among the blocks mapped errors before a
    // single row goes out; one past an early stop is never seen (`KD6`).
    let mut matches: Vec<CopyBlock> = index
        .blocks_for(table)
        .filter(|b| selector.is_none() || b.database.as_deref() == selector)
        .cloned()
        .collect();
    let mut target: Option<(Option<String>, String)> = None;
    for b in &matches {
        let key = (b.database.clone(), b.header.qualified_name());
        match &target {
            None => target = Some(key),
            Some(t) if *t != key => {
                return Err(Error::AmbiguousTable {
                    name: table.to_string(),
                    candidates: vec![render_candidate(t), render_candidate(&key)],
                });
            }
            _ => {}
        }
    }

    census_metadata_level(
        source,
        scan_options,
        query_options,
        cache,
        metadata.as_ref(),
        &mut matches,
        index.scanned_through,
        size,
    )
    .await?;

    // **A streamed schema needs no completeness test**
    // (`docs/design/decisions.md`, "D34"): the mapping pass has finished and
    // `matches` is fixed, so the census the plan unions over them is the
    // evidence for exactly the rows this stream will hand back
    // (`docs/design/decisions.md`, "D35").
    Ok(MappedTable { matches, metadata })
}

/// **A query's cold semantics over a table the map holds at the metadata
/// level** (`docs/design/decisions.md`, "D35"): each of `matches` holding no
/// census is read again for one, as a query's own mapping pass would have
/// censused it, and holds it for this query alone — the map and the cache
/// keep the level `parse` gave them, so the price is a second read of the
/// table on every such query. Under [`SchemaMode::Strings`], which reads no
/// census, nothing is read. A cancelled read is
/// [`Error::ScanCancelled`] at `scanned_through`, as a cancelled mapping pass
/// is.
#[allow(clippy::too_many_arguments)]
async fn census_metadata_level(
    source: &dyn ByteRangeSource,
    scan_options: &ScanOptions,
    query_options: &QueryOptions,
    cache: &CacheMode,
    metadata: Option<&DumpMetadata>,
    matches: &mut [CopyBlock],
    scanned_through: u64,
    size: u64,
) -> Result<()> {
    if query_options.schema_mode != SchemaMode::Typed {
        return Ok(());
    }
    let lacking: Vec<usize> =
        (0..matches.len()).filter(|&i| matches[i].array_shapes.is_none()).collect();
    let Some(&first) = lacking.first() else { return Ok(()) };
    tracing::info!(
        table = matches[first].header.qualified_name(),
        blocks = lacking.len(),
        "table mapped at the metadata level: reading its rows again for this query's census",
    );
    announce_read_loop(source, scan_options);
    let cache_path = match cache {
        CacheMode::Enabled { path, .. } => Some(path.as_path()),
        CacheMode::Disabled { .. } | CacheMode::Offline(_) => None,
    };
    let mut shortfall_reported = false;
    for at in lacking {
        let read = reread_rows(
            source,
            scan_options,
            cache_path,
            metadata,
            &matches[at],
            size,
            None,
            &mut shortfall_reported,
        )
        .await?;
        let Some(census) = read else {
            return Err(Error::ScanCancelled { scanned_through });
        };
        matches[at].set_census(census);
    }
    Ok(())
}

/// `database`, rendered as `database.schema.table`, or just `schema.table`
/// when the file had no `\connect` at all — the form `Error::AmbiguousTable`
/// names its candidates in.
fn render_candidate((database, qualified_name): &(Option<String>, String)) -> String {
    match database {
        Some(db) => format!("{db}.{qualified_name}"),
        None => qualified_name.clone(),
    }
}
