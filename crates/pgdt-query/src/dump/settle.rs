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
use pgdt_io::{ByteRangeSource, Extent};
use pgdt_mapping::internal::{MapStop, Pass, Target, map_forward, reread_rows, splice};
use pgdt_model::timescaledb::TimescaleDb;
use pgdt_model::{CopyBlock, DumpIndex, DumpMetadata, TableName};
use tokio_util::sync::CancellationToken;

use super::descendants::{TableRead, declared_name, listed_tables, table_read};
use super::{Table, TableSelector, select};
use crate::options::ScanExtent;

/// What the pass settled: the table, what a read of it reads, and the DDL to
/// type its blocks against.
type Settled = (TableName, TableRead, Option<Arc<DumpMetadata>>);

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
    /// **It learns no [`Inheritance`](crate::Inheritance) mode**: a table any
    /// descendant is declared for is settled with them, so the table it hands
    /// back is read under either, as a complete map's is
    /// (`docs/design/decisions.md`, "D132").
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
        Ok(settled.map(|(name, TableRead { blocks, through, refused, notes }, metadata)| Table {
            source,
            watch,
            metadata,
            name,
            blocks,
            through,
            refused,
            notes,
        }))
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
        let source_extent = source.extent().await?;
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
            // Asked again: a source whose end was open at the start may have
            // met it in the preamble's reads.
            let end = source.extent().await?.exact();
            index.spans = splice(&[], spans, 0, preamble_end, end);
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
            ScanExtent::UntilTargetSettled if !reads_descendants(&index, selector) => {
                Some(&selects)
            }
            ScanExtent::UntilTargetSettled | ScanExtent::Full => None,
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
            source_extent,
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
        let tables = listed_tables(&index);
        let Some(name) = select(&tables, selector)? else { return Ok(None) };
        let mut read = table_read(&index, name);
        if schema_mode == SchemaMode::Typed && read.refused.reads_either() {
            let database = name.database.as_deref();
            // A table holding no block settles its columns, read alone, from
            // the blocks written through its root (`Table::schema`).
            let blocks = match read.blocks.is_empty() {
                true => &mut read.through,
                false => &mut read.blocks,
            };
            self.census(metadata.as_deref(), database, blocks, source_extent).await?;
        }
        // **A streamed schema needs no completeness test**
        // (`docs/design/decisions.md`, "D34"): the pass has finished and the
        // blocks are fixed, so the census the plan unions over them is the
        // evidence for exactly the rows its streams will hand back
        // (`docs/design/decisions.md`, "D35").
        Ok(Some((name.clone(), read, metadata)))
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
        extent: Extent,
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
                extent,
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

/// Whether a table `selector` selects in the DDL `index` holds has a
/// descendant, whose blocks may lie anywhere past the table's own, so the
/// pass settling it runs to the file's end (`docs/design/decisions.md`,
/// "D131") — or, in a TimescaleDB database, an ancestor: it may be a chunk,
/// whose catalog blocks, which say whether its rows are compressed, may lie
/// past it too (`docs/design/decisions.md`, "D134"). A database the map has
/// not reached is a `\connect`'s, which runs the pass to the end already
/// (`docs/design/decisions.md`, "D49").
///
/// deficiency: KD160 — a database is TimescaleDB's here by the extension its
/// DDL creates or a catalog block the map already holds, so a dump holding
/// the catalog without the extension (`--table`, TS2) whose chunk's block
/// precedes the catalog's — its schema sorting before
/// `_timescaledb_catalog` — settles the chunk before its catalog is read:
/// compressed, it reads its own block alone where it should refuse
/// (`KD159`). The closure needing no new invariant runs every `INHERITS`
/// child's cold pass to the end in any database, a chunk being one, which
/// moves the cost onto every inheritance child. **(c) unowned**; promoted by
/// such a dump met in use.
///
/// deficiency: KD161 — a chunk's cold pass maps to the file's end wherever
/// its catalog lies, though the catalog precedes it wherever its schema sorts
/// after `_timescaledb_catalog`. A stop trusting no order is known: once the
/// chunk's block is mapped and the catalog's verdict can no longer change,
/// which bounds only a catalog stating its release (TS3) with each table its
/// recipe reads mapped, an inferred era reading every table's columns. It is
/// a stop decided over the index, beside the block-header [`Target`]
/// (`docs/design/decisions.md`, "D49"). A file with a `\connect` never stops
/// early, so pays nothing, and a cached map pays it once. **(c) unowned**;
/// promoted by a cold query by a chunk's name met in use.
// timescaledb: TS1 — a chunk is an inheritance child of its hypertable.
fn reads_descendants(index: &DumpIndex, selector: &TableSelector) -> bool {
    index.metadata.as_deref().is_some_and(|metadata| {
        metadata.databases.iter().any(|db| {
            let timescaledb = TimescaleDb::of(index, db.name.as_deref()).is_some();
            db.tables.keys().any(|key| {
                selector.selects(&declared_name(db.name.as_deref(), key))
                    && (!db.descendants(key).is_empty()
                        || timescaledb && !db.ancestors(key).is_empty())
            })
        })
    })
}
