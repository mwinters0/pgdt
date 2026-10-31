//! The release a catalog states, or the era its shape bounds, and the recipe
//! the release decides ([`Recipe`]).

use std::fmt;

use serde::Serialize;

use super::catalog::Catalog;

/// A TimescaleDB release, to its minor: what a catalog's shape changes at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Release {
    pub major: u32,
    pub minor: u32,
}

impl Release {
    /// Release `major.minor`.
    pub const fn new(major: u32, minor: u32) -> Self {
        Release { major, minor }
    }

    /// The release a `major.minor[.patch][-suffix]` version names.
    fn parse(text: &str) -> Option<Release> {
        let mut parts = text.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor: String = parts.next()?.chars().take_while(char::is_ascii_digit).collect();
        Some(Release { major, minor: minor.parse().ok()? })
    }
}

impl fmt::Display for Release {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// The releases a catalog's shape is consistent with: `from` through
/// `through`, an open end where nothing it holds has gone since and nothing
/// it lacks has arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Era {
    pub from: Release,
    pub through: Option<Release>,
}

/// The release a catalog states, or what its shape says of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum Version {
    /// The `metadata` row `timescaledb_version`, from 2.14 (TS3).
    Stated { version: String },
    /// No version row: the era the catalog's columns and tables bound (TS3).
    Inferred { era: Era },
    /// No version row, and columns and tables no release holds together.
    Unknown,
}

impl Version {
    /// Whether the release is known to be `release` or later: the one stated,
    /// or every one of the era its shape bounds. An unknown release is none.
    pub fn at_least(&self, release: Release) -> bool {
        match self {
            Version::Stated { version } => Release::parse(version).is_some_and(|r| r >= release),
            Version::Inferred { era } => era.from >= release,
            Version::Unknown => false,
        }
    }
}

/// The way a chunk is joined to its table, its ranges and its compressed
/// relation, as the catalog's shape changed (TS3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Recipe {
    /// Through 2.27: `chunk.schema_name` and `table_name`, its ranges through
    /// `chunk_constraint`, its compressed relation `chunk.compressed_chunk_id`.
    ChunkConstraint,
    /// 2.28: as before, its ranges `dimension_slice.chunk_id`.
    SliceChunkId,
    /// From 2.29: `chunk.relid`, `dimension_slice.chunk_id`, its compressed
    /// relation `compression_settings.compress_relid`.
    Relid,
}

/// The oldest release pgdt infers a catalog to be: the marks below are read
/// from 2.0's releases on, and no fixture holds a 1.x catalog, though a 1.x
/// server is one a routine `pg_dump` dumps.
const FLOOR: Release = Release::new(2, 0);

/// The releases a shape allows: a first, a last, either open.
type Bounds = (Option<Release>, Option<Release>);

/// What one column's presence or absence in a recorded catalog says of its
/// release: present, the era `present` bounds; absent, `absent`'s.
struct ColumnMark {
    table: &'static str,
    column: &'static str,
    present: Bounds,
    absent: Bounds,
}

const fn mark(
    table: &'static str,
    column: &'static str,
    present: Bounds,
    absent: Bounds,
) -> ColumnMark {
    ColumnMark { table, column, present, absent }
}

const fn since(major: u32, minor: u32) -> Bounds {
    (Some(Release::new(major, minor)), None)
}

const fn until(major: u32, minor: u32) -> Bounds {
    (None, Some(Release::new(major, minor)))
}

/// The columns whose coming and going bound a catalog's release.
const COLUMN_MARKS: [ColumnMark; 7] = [
    mark("chunk", "dropped", until(2, 25), since(2, 26)),
    mark("chunk", "status", since(2, 3), until(2, 2)),
    mark("chunk", "osm_chunk", since(2, 8), until(2, 7)),
    mark("chunk", "creation_time", since(2, 13), until(2, 12)),
    mark("chunk", "relid", since(2, 29), until(2, 28)),
    mark("hypertable", "replication_factor", until(2, 13), since(2, 14)),
    mark("dimension_slice", "chunk_id", since(2, 28), until(2, 27)),
];

/// The tables whose presence bounds a catalog's release. Absence says
/// nothing, a `--table` dump selecting what it likes (TS2).
const TABLE_MARKS: [(&str, Bounds); 3] = [
    ("hypertable_compression", until(2, 13)),
    ("compression_settings", since(2, 14)),
    ("chunk_constraint", until(2, 27)),
];

/// The release `catalog` states, else the era its shape bounds. The DDL's
/// markers (`ts_insert_blocker`, the function schema, the extension comment)
/// are not read: [`recipe`] reads no era, and none of them moves an era's
/// start to 2.28, the one bound a query reads ([`Version::at_least`], for a
/// root's rows read alone), so they would move what `pgdt info` shows and,
/// short of a dump contradicting its own catalog, nothing a query reads.
// timescaledb: TS3 — the version row from 2.14, the catalog's columns before it.
pub(super) fn version(catalog: &Catalog<'_>) -> Version {
    let stated = catalog.rows("metadata").find_map(|row| match row.text("key") {
        Ok(Some("timescaledb_version")) => row.text("value").ok().flatten(),
        _ => None,
    });
    if let Some(version) = stated {
        return Version::Stated { version: version.to_owned() };
    }
    let mut from = FLOOR;
    let mut through: Option<Release> = None;
    let mut bound = |(lo, hi): Bounds| {
        from = from.max(lo.unwrap_or(FLOOR));
        through = match (through, hi) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
    };
    for m in &COLUMN_MARKS {
        match catalog.has_column(m.table, m.column) {
            Some(true) => bound(m.present),
            Some(false) => bound(m.absent),
            None => {}
        }
    }
    for (table, era) in TABLE_MARKS {
        if catalog.has_table(table) {
            bound(era);
        }
    }
    match through {
        Some(through) if through < from => Version::Unknown,
        _ => Version::Inferred { era: Era { from, through } },
    }
}

/// The recipe `version` decides: by the release stated where it parses, else
/// by the shape that tells the three apart — `chunk.relid`, then
/// `dimension_slice.chunk_id`.
pub(super) fn recipe(version: &Version, catalog: &Catalog<'_>) -> Recipe {
    let stated = match version {
        Version::Stated { version } => Release::parse(version),
        Version::Inferred { .. } | Version::Unknown => None,
    };
    match stated {
        Some(release) if release >= Release::new(2, 29) => Recipe::Relid,
        Some(release) if release >= Release::new(2, 28) => Recipe::SliceChunkId,
        Some(_) => Recipe::ChunkConstraint,
        None if catalog.has_column("chunk", "relid") == Some(true) => Recipe::Relid,
        None if catalog.has_column("dimension_slice", "chunk_id") == Some(true) => {
            Recipe::SliceChunkId
        }
        None => Recipe::ChunkConstraint,
    }
}
