//! A cold query's mapping pass, whole: the map an open's cache holds
//! extended until one table is settled, the census a metadata-level table
//! lacks read for it, and that table handed back as the [`Table`] a complete
//! map's would be (`docs/design/decisions.md`, "D48", "D49").

use std::sync::Arc;

use pgdt_codec::SchemaMode;
#[cfg(feature = "introspect")]
use pgdt_core::internal::statistics_loaded;
use pgdt_core::{CopyHeader, Error, Result};
use pgdt_dump::cache::{CacheMode, Held, Opened, SourceWatch};
use pgdt_dump::internal::{announce_read_loop, scan_preamble};
use pgdt_dump::{ScanOptions, attach_text};
use pgdt_io::ByteRangeSource;
use pgdt_mapping::internal::{MapStop, Pass, Target, map_forward, reread_rows, splice};
use pgdt_model::{CopyBlock, DumpIndex, DumpMetadata, TableName};
use tokio_util::sync::CancellationToken;

use super::{Table, TableSelector, select};
use crate::options::ScanExtent;

/// What the pass settled: the table, its blocks, and the DDL to type them
/// against.
type Settled = (TableName, Vec<CopyBlock>, Option<Arc<DumpMetadata>>);

impl Table {
    /// **A cold query's one table**: the dump `opened` opened, its map
    /// extended from wherever its cache stops — under `scan`, saving to the
    /// open's mode as it goes — until the table `selector` selects is
    /// settled, or to the file's end under [`ScanExtent::Full`]; then that
    /// table, as [`Dump::table`](crate::Dump::table) would hand it back over a
    /// complete map, sharing the open's source and watch. `None` where the
    /// map reached selects no table, and [`Error::AmbiguousTable`] where it
    /// selects several, naming each.
    ///
    /// **It maps and reads no row**: rows come from the plan's streams, the
    /// table's bytes read again there (`docs/design/decisions.md`, "D48").
    /// The first database's preamble is captured first, whatever is selected
    /// (`docs/design/decisions.md`, "D30"). Under [`ScanExtent::UntilTargetSettled`]
    /// a conflicting table past where the pass stopped is never seen
    /// (`KD6`, at [`ScanExtent`]).
    ///
    /// **A table the map holds at the metadata level is read again for its
    /// census** under [`SchemaMode::Typed`], as this pass would have censused
    /// it, which the table holds for this query alone — the map and the cache
    /// keep the level a parse gave them, so the price is a second read of the
    /// table each time (`docs/design/decisions.md`, "D35"). A table settled
    /// under [`SchemaMode::Strings`] holds none, and its plan under
    /// [`SchemaMode::Typed`] refuses as a complete map's does.
    ///
    /// **`cancel` is the caller's ask that the pass stop**, polled as a
    /// mapping pass polls it (`docs/design/decisions.md`, "D26"): a pass it
    /// stops saves what it holds and answers [`Error::ScanCancelled`], the
    /// table being unsettled. A caller that never stops one passes a token it
    /// never fires. It is not handed to the source, so a remote read in flight
    /// finishes before the next chunk's poll.
    pub async fn settle(
        opened: Opened,
        selector: &TableSelector,
        scan: ScanOptions,
        extent: ScanExtent,
        schema_mode: SchemaMode,
        cancel: &CancellationToken,
    ) -> Result<Option<Table>> {
        let Opened { source, watch, cache, mode, .. } = opened;
        watch.require_checkable(source.as_ref())?;
        let pass = ColdPass { source: source.as_ref(), watch: &watch, mode: &mode, scan, cancel };
        let settled = pass.settle(cache, selector, extent, schema_mode).await;
        let settled = watch.attribute(source.as_ref(), settled).await?;
        let watch = Arc::new(watch);
        Ok(settled.map(|(name, blocks, metadata)| Table { source, watch, metadata, name, blocks }))
    }
}

/// What the pass reads under: the open's source, watch and mode, and the
/// caller's arrangement and token.
struct ColdPass<'a> {
    source: &'a dyn ByteRangeSource,
    watch: &'a SourceWatch,
    mode: &'a CacheMode,
    scan: ScanOptions,
    cancel: &'a CancellationToken,
}

impl ColdPass<'_> {
    /// [`Table::settle`] once its watch is open, which then attributes any
    /// failure: building forward from what the open held.
    async fn settle(
        &self,
        held: Held,
        selector: &TableSelector,
        extent: ScanExtent,
        schema_mode: SchemaMode,
    ) -> Result<Option<Settled>> {
        let (source, watch, mode) = (self.source, self.watch, self.mode);
        let size = source.size().await?;
        let mut index = match held {
            Held::Loaded(loaded) => loaded.index,
            // Nothing to build forward from, or a cache the mode may replace,
            // which the first save writes over (`docs/design/decisions.md`, "D20").
            Held::Missing | Held::Disabled | Held::Replacing(_) => DumpIndex::default(),
        };
        // The one statistics term a query has: it keeps no account, so nothing
        // else says what the cache handed it.
        #[cfg(feature = "introspect")]
        statistics_loaded(index.statistics_heap_bytes());

        // The first database's preamble is captured before anything else runs,
        // whatever table this call settles (`docs/design/decisions.md`, "D30").
        // A disabled mode still runs the scan, its save a no-op. Persisted
        // immediately, so it survives a caller that drops the pass after it.
        let first_db_preamble_known = index
            .metadata
            .as_ref()
            .and_then(|m| m.databases.first())
            .is_some_and(|db| db.preamble_complete);
        if !first_db_preamble_known {
            // The prepass's spans are kept, not discarded: they tile
            // `[0, preamble_end)`, which is the prefix `map_forward` splices
            // its own output onto.
            let (metadata, spans, preamble_end, roles, tablespaces) =
                scan_preamble(source, &self.scan).await?;
            index.metadata = Some(Arc::new(metadata));
            index.spans = splice(&[], spans, 0, preamble_end, size);
            index.roles.extend(roles);
            index.tablespaces.extend(tablespaces);
            index.scanned_through = index.scanned_through.max(preamble_end);
            attach_text(source, &mut index.spans).await?;
            mode.save(watch, source, &index).await?;
        }
        // Extend the map until the selected table is settled. No rows come
        // out of this.
        let selects = |database: Option<&str>, header: &CopyHeader| {
            selector.selects_block(database, header.schema.as_deref(), &header.table)
        };
        let target: Option<Target<'_>> = match extent {
            ScanExtent::UntilTargetSettled => Some(&selects),
            ScanExtent::Full => None,
        };
        let stop = map_forward(
            source,
            &self.scan,
            self.cancel,
            mode,
            watch,
            &mut index,
            target,
            Pass::Query,
            size,
        )
        .await?;
        if stop == MapStop::Interrupted {
            return Err(Error::ScanCancelled { scanned_through: index.scanned_through });
        }

        // Read *after* the mapping pass, not before it: `map_forward` states
        // the metadata at each `\connect`ed database's first `COPY` block, so
        // the schema depends on the map and never on how far a replay has got.
        let metadata = index.metadata.clone();

        // One target per query (`docs/design/decisions.md`, "D49"): the
        // selector narrows the tables mapped to at most one before any block
        // is read again, so a would-be silent union across schemas or
        // databases errors instead. An ambiguous name among the tables mapped
        // errors before a single row goes out; one past an early stop is never
        // seen (`KD6`).
        let tables = index.tables();
        let Some(name) = select(&tables, selector)? else { return Ok(None) };
        let mut blocks: Vec<CopyBlock> = index.blocks_of(name).cloned().collect();
        if schema_mode == SchemaMode::Typed {
            self.census(metadata.as_deref(), name.database.as_deref(), &mut blocks, size).await?;
        }
        // **A streamed schema needs no completeness test**
        // (`docs/design/decisions.md`, "D34"): the pass has finished and the
        // blocks are fixed, so the census the plan unions over them is the
        // evidence for exactly the rows its streams will hand back
        // (`docs/design/decisions.md`, "D35").
        Ok(Some((name.clone(), blocks, metadata)))
    }

    /// **A query's cold semantics over a table the map holds at the metadata
    /// level** (`docs/design/decisions.md`, "D35"): each of `blocks` holding
    /// no census is read again for one, as this pass would have censused it,
    /// and holds it for this query alone.
    async fn census(
        &self,
        metadata: Option<&DumpMetadata>,
        database: Option<&str>,
        blocks: &mut [CopyBlock],
        size: u64,
    ) -> Result<()> {
        let lacking: Vec<usize> =
            (0..blocks.len()).filter(|&i| blocks[i].census.is_none()).collect();
        let Some(&first) = lacking.first() else { return Ok(()) };
        tracing::info!(
            table = blocks[first].header.qualified_name(),
            blocks = lacking.len(),
            "table mapped at the metadata level: reading its rows again for this query's census",
        );
        announce_read_loop(self.source, self.scan.read, self.scan.parallelism);
        let mut shortfall_reported = false;
        for at in lacking {
            let read = reread_rows(
                self.source,
                &self.scan,
                self.cancel,
                self.mode.path(),
                metadata,
                database,
                &blocks[at],
                size,
                None,
                &mut shortfall_reported,
            )
            .await?;
            let Some(census) = read else {
                return Err(Error::ScanCancelled { scanned_through: blocks[at].header_offset });
            };
            blocks[at].census = Some(census);
        }
        Ok(())
    }
}
