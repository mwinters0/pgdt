//! A catalog read by its [`Recipe`]: the hypertables, their dimensions and
//! chunks, each chunk's ranges, status and compressed relation, and the
//! continuous aggregates.

use std::collections::BTreeMap;

use serde::Serialize;

use super::catalog::{Catalog, Row, regclass_name};
use super::version::Recipe;

/// What a readable catalog holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Resolved {
    pub recipe: Recipe,
    /// In `id` order, the internal compressed hypertables included.
    pub hypertables: Vec<Hypertable>,
    /// `None` where the catalog's `continuous_agg` lacks a column read
    /// here, empty where it holds no such block.
    pub continuous_aggregates: Option<Vec<ContinuousAggregate>>,
}

/// One hypertable, as `schema.table`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Hypertable {
    pub id: i64,
    pub table: String,
    /// An internal compressed hypertable (`compression_state = 2`), whose
    /// chunks hold another hypertable's compressed rows: no user table.
    pub internal: bool,
    pub dimensions: Vec<Dimension>,
    /// In `id` order, a dropped chunk's row included.
    pub chunks: Vec<Chunk>,
}

/// One dimension of a hypertable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Dimension {
    pub id: i64,
    pub column: String,
    /// A closed dimension, sliced by hash (`num_slices` set).
    pub hashed: bool,
}

/// One chunk, its table as `schema.table`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Chunk {
    pub id: i64,
    pub table: String,
    /// `chunk.status`'s bits, or what a catalog before the column implies.
    pub status: u32,
    /// A row `drop_chunks` kept for a table it dropped (TS6): no chunk.
    pub dropped: bool,
    /// The relation holding its compressed rows, as `schema.table`.
    pub compressed_relation: Option<String>,
    /// Its range in each dimension.
    pub slices: Vec<Slice>,
}

impl Chunk {
    /// Status bit 1: a compressed relation holds rows of it (TS4).
    // timescaledb: TS4 — bit 1.
    pub fn compressed(&self) -> bool {
        self.status & 1 != 0
    }

    /// Status bit 8: rows inserted after its compression are in its own
    /// table (TS4).
    // timescaledb: TS4 — bit 8.
    pub fn partially_compressed(&self) -> bool {
        self.status & 8 != 0
    }
}

/// A chunk's half-open range in one dimension, as the catalog states it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Slice {
    pub dimension: i64,
    pub start: i64,
    pub end: i64,
}

/// One continuous aggregate (TS7): its view, and the hypertables it reads
/// from and materializes into, as `schema.table` where the catalog lists
/// them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContinuousAggregate {
    pub view: String,
    pub materialization: Option<String>,
    pub raw: Option<String>,
    pub materialized_only: bool,
}

/// Why a catalog cannot be read by its recipe.
#[derive(Debug, Default)]
pub(super) struct Fault {
    pub missing: Vec<String>,
    pub malformed: Vec<String>,
}

/// The columns each recipe reads, by table.
fn needed(recipe: Recipe) -> Vec<(&'static str, &'static [&'static str])> {
    const NAMED_CHUNK: &[&str] =
        &["id", "hypertable_id", "schema_name", "table_name", "compressed_chunk_id"];
    const SLICE_OWNS: &[&str] = &["id", "chunk_id", "dimension_id", "range_start", "range_end"];
    let mut needed: Vec<(&str, &[&str])> = vec![
        ("hypertable", &["id", "schema_name", "table_name", "compression_state"]),
        ("dimension", &["id", "hypertable_id", "column_name"]),
    ];
    match recipe {
        Recipe::ChunkConstraint => needed.extend([
            ("chunk", NAMED_CHUNK),
            ("dimension_slice", &["id", "dimension_id", "range_start", "range_end"][..]),
            ("chunk_constraint", &["chunk_id", "dimension_slice_id"][..]),
        ]),
        Recipe::SliceChunkId => {
            needed.extend([("chunk", NAMED_CHUNK), ("dimension_slice", SLICE_OWNS)])
        }
        Recipe::Relid => needed.extend([
            ("chunk", &["id", "relid", "hypertable_id"][..]),
            ("dimension_slice", SLICE_OWNS),
            ("compression_settings", &["relid", "compress_relid"][..]),
        ]),
    }
    needed
}

/// Reads a row's fields, recording each that is missing or does not parse.
struct Reader<'f> {
    fault: &'f mut Fault,
}

impl Reader<'_> {
    fn text<'a>(&mut self, row: &Row<'a>, column: &str) -> Option<&'a str> {
        match row.text(column) {
            Ok(text) => text,
            Err(name) => {
                self.malformed(name);
                None
            }
        }
    }

    /// A column the catalog declares `NOT NULL`, read as an integer.
    fn int(&mut self, row: &Row<'_>, column: &str) -> Option<i64> {
        let text = self.required(row, column)?;
        text.parse().ok().or_else(|| {
            self.malformed(format!("{}.{column}", row.table));
            None
        })
    }

    /// A nullable integer column: `Some(None)` for a NULL.
    fn nullable_int(&mut self, row: &Row<'_>, column: &str) -> Option<Option<i64>> {
        match self.text(row, column) {
            None => Some(None),
            Some(text) => match text.parse() {
                Ok(n) => Some(Some(n)),
                Err(_) => {
                    self.malformed(format!("{}.{column}", row.table));
                    None
                }
            },
        }
    }

    fn required<'a>(&mut self, row: &Row<'a>, column: &str) -> Option<&'a str> {
        let text = self.text(row, column);
        if text.is_none() {
            self.malformed(format!("{}.{column}", row.table));
        }
        text
    }

    fn malformed(&mut self, name: String) {
        if !self.fault.malformed.contains(&name) {
            self.fault.malformed.push(name);
        }
    }
}

/// `catalog` read by `recipe`, or the columns it lacks and the fields that
/// do not read.
// timescaledb: TS5 — a chunk's compressed relation, by the link each recipe states.
pub(super) fn resolve(recipe: Recipe, catalog: &Catalog<'_>) -> Result<Resolved, Fault> {
    let mut fault = Fault::default();
    for (table, columns) in needed(recipe) {
        for column in columns {
            if catalog.has_column(table, column) != Some(true) {
                fault.missing.push(format!("{table}.{column}"));
            }
        }
    }
    if !fault.missing.is_empty() {
        return Err(fault);
    }
    let mut read = Reader { fault: &mut fault };

    let mut hypertables: BTreeMap<i64, Hypertable> = BTreeMap::new();
    for row in catalog.rows("hypertable") {
        let (Some(id), Some(schema), Some(table), Some(state)) = (
            read.int(&row, "id"),
            read.required(&row, "schema_name"),
            read.required(&row, "table_name"),
            read.int(&row, "compression_state"),
        ) else {
            continue;
        };
        let table = format!("{schema}.{table}");
        let internal = state == 2;
        hypertables
            .insert(id, Hypertable { id, table, internal, dimensions: vec![], chunks: vec![] });
    }
    let hashed_known = catalog.has_column("dimension", "num_slices") == Some(true);
    for row in catalog.rows("dimension") {
        let (Some(id), Some(hypertable), Some(column)) = (
            read.int(&row, "id"),
            read.int(&row, "hypertable_id"),
            read.required(&row, "column_name"),
        ) else {
            continue;
        };
        let hashed = hashed_known && read.text(&row, "num_slices").is_some();
        match hypertables.get_mut(&hypertable) {
            Some(h) => h.dimensions.push(Dimension { id, column: column.to_owned(), hashed }),
            None => read.malformed("dimension.hypertable_id".to_owned()),
        }
    }

    // Each slice by id, with the chunk it names from 2.28.
    let mut slices: BTreeMap<i64, (Slice, Option<i64>)> = BTreeMap::new();
    for row in catalog.rows("dimension_slice") {
        let chunk = match recipe {
            Recipe::ChunkConstraint => None,
            Recipe::SliceChunkId | Recipe::Relid => match read.int(&row, "chunk_id") {
                Some(chunk) => Some(chunk),
                None => continue,
            },
        };
        let (Some(id), Some(dimension), Some(start), Some(end)) = (
            read.int(&row, "id"),
            read.int(&row, "dimension_id"),
            read.int(&row, "range_start"),
            read.int(&row, "range_end"),
        ) else {
            continue;
        };
        slices.insert(id, (Slice { dimension, start, end }, chunk));
    }

    let chunks = read_chunks(recipe, catalog, &mut read);
    let names: BTreeMap<i64, String> = chunks.iter().map(|c| (c.0.id, c.0.table.clone())).collect();
    let compressed_by_settings: BTreeMap<String, String> = match recipe {
        Recipe::Relid => catalog
            .rows("compression_settings")
            .filter_map(|row| {
                let relid = regclass_name(read.text(&row, "relid")?)?;
                let compressed = regclass_name(read.text(&row, "compress_relid")?)?;
                Some((relid, compressed))
            })
            .collect(),
        Recipe::ChunkConstraint | Recipe::SliceChunkId => BTreeMap::new(),
    };
    let mut chunk_slices: BTreeMap<i64, Vec<Slice>> = BTreeMap::new();
    match recipe {
        Recipe::ChunkConstraint => {
            for row in catalog.rows("chunk_constraint") {
                let (Some(chunk), Some(Some(slice))) =
                    (read.int(&row, "chunk_id"), read.nullable_int(&row, "dimension_slice_id"))
                else {
                    continue;
                };
                match slices.get(&slice) {
                    Some((slice, _)) => chunk_slices.entry(chunk).or_default().push(*slice),
                    None => read.malformed("chunk_constraint.dimension_slice_id".to_owned()),
                }
            }
        }
        Recipe::SliceChunkId | Recipe::Relid => {
            for (slice, chunk) in slices.values() {
                if let Some(chunk) = chunk {
                    chunk_slices.entry(*chunk).or_default().push(*slice);
                }
            }
        }
    }

    for (mut chunk, hypertable, compressed_chunk, status) in chunks {
        chunk.compressed_relation = match recipe {
            Recipe::Relid => compressed_by_settings.get(&chunk.table).cloned(),
            Recipe::ChunkConstraint | Recipe::SliceChunkId => {
                compressed_chunk.and_then(|id| names.get(&id).cloned())
            }
        };
        if compressed_chunk.is_some_and(|id| !names.contains_key(&id)) {
            read.malformed("chunk.compressed_chunk_id".to_owned());
        }
        chunk.status = status_of(status, &chunk);
        chunk.slices = chunk_slices.remove(&chunk.id).unwrap_or_default();
        chunk.slices.sort_by_key(|slice| slice.dimension);
        match hypertables.get_mut(&hypertable) {
            Some(h) => h.chunks.push(chunk),
            None => read.malformed("chunk.hypertable_id".to_owned()),
        }
    }

    let continuous_aggregates = continuous_aggregates(catalog, &hypertables, &mut read);
    if !fault.malformed.is_empty() {
        return Err(fault);
    }
    let hypertables = hypertables
        .into_values()
        .map(|mut h| {
            h.chunks.sort_by_key(|c| c.id);
            h
        })
        .collect();
    Ok(Resolved { recipe, hypertables, continuous_aggregates })
}

/// The hypertables `catalog`'s `hypertable` rows name, as `schema.table`, by
/// those two columns alone: what an unreadable catalog still says of which
/// tables are hypertables. `None` where it holds no `hypertable` block naming
/// both, or a row with either NULL.
pub(super) fn hypertable_names(catalog: &Catalog<'_>) -> Option<Vec<String>> {
    for column in ["schema_name", "table_name"] {
        if catalog.has_column("hypertable", column) != Some(true) {
            return None;
        }
    }
    catalog
        .rows("hypertable")
        .map(|row| {
            let schema = row.text("schema_name").ok()??;
            let table = row.text("table_name").ok()??;
            Some(format!("{schema}.{table}"))
        })
        .collect()
}

/// A chunk, its hypertable's id, the id of the chunk row holding its
/// compressed rows (through 2.28) and its `status` where the catalog has
/// the column.
type ChunkRow = (Chunk, i64, Option<i64>, Option<u32>);

// timescaledb: TS6 — a `dropped` row is a chunk's row whose table is gone.
fn read_chunks(recipe: Recipe, catalog: &Catalog<'_>, read: &mut Reader<'_>) -> Vec<ChunkRow> {
    let has_status = catalog.has_column("chunk", "status") == Some(true);
    let has_dropped = catalog.has_column("chunk", "dropped") == Some(true);
    let mut chunks = Vec::new();
    for row in catalog.rows("chunk") {
        let (Some(id), Some(hypertable)) = (read.int(&row, "id"), read.int(&row, "hypertable_id"))
        else {
            continue;
        };
        let (table, compressed_chunk) = match recipe {
            Recipe::Relid => {
                let Some(relid) = read.required(&row, "relid") else { continue };
                let Some(table) = regclass_name(relid) else {
                    read.malformed("chunk.relid".to_owned());
                    continue;
                };
                (table, None)
            }
            Recipe::ChunkConstraint | Recipe::SliceChunkId => {
                let (Some(schema), Some(name), Some(compressed)) = (
                    read.required(&row, "schema_name"),
                    read.required(&row, "table_name"),
                    read.nullable_int(&row, "compressed_chunk_id"),
                ) else {
                    continue;
                };
                (format!("{schema}.{name}"), compressed)
            }
        };
        let status = match has_status {
            true => match read.int(&row, "status").map(u32::try_from) {
                Some(Ok(status)) => Some(status),
                Some(Err(_)) => {
                    read.malformed("chunk.status".to_owned());
                    continue;
                }
                None => continue,
            },
            false => None,
        };
        let dropped = has_dropped && read.text(&row, "dropped") == Some("t");
        let chunk =
            Chunk { id, table, status: 0, dropped, compressed_relation: None, slices: vec![] };
        chunks.push((chunk, hypertable, compressed_chunk, status));
    }
    chunks
}

/// A chunk's status: the catalog's, or, before the column (2.3), bit 1
/// where it names a compressed relation.
// timescaledb: TS4 — bit 1 compressed, bit 8 partially compressed.
fn status_of(status: Option<u32>, chunk: &Chunk) -> u32 {
    status.unwrap_or(u32::from(chunk.compressed_relation.is_some()))
}

// timescaledb: TS7 — an aggregate's rows are its materialization hypertable's.
fn continuous_aggregates(
    catalog: &Catalog<'_>,
    hypertables: &BTreeMap<i64, Hypertable>,
    read: &mut Reader<'_>,
) -> Option<Vec<ContinuousAggregate>> {
    const READ: [&str; 5] = [
        "mat_hypertable_id",
        "raw_hypertable_id",
        "user_view_schema",
        "user_view_name",
        "materialized_only",
    ];
    match catalog.has_column("continuous_agg", READ[0]) {
        None => return Some(vec![]),
        Some(_) if READ.iter().any(|c| catalog.has_column("continuous_agg", c) != Some(true)) => {
            return None;
        }
        Some(_) => {}
    }
    let name = |id: Option<i64>| id.and_then(|id| hypertables.get(&id)).map(|h| h.table.clone());
    let mut aggregates = Vec::new();
    for row in catalog.rows("continuous_agg") {
        let (Some(schema), Some(view)) =
            (read.required(&row, "user_view_schema"), read.required(&row, "user_view_name"))
        else {
            continue;
        };
        aggregates.push(ContinuousAggregate {
            view: format!("{schema}.{view}"),
            materialization: name(read.int(&row, "mat_hypertable_id")),
            raw: name(read.int(&row, "raw_hypertable_id")),
            materialized_only: read.text(&row, "materialized_only") == Some("t"),
        });
    }
    Some(aggregates)
}
