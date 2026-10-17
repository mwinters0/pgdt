//! A dump opened through its complete cache (`docs/design/decisions.md`,
//! "D90").

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use pgdt_codec::{SchemaMode, UnrepresentableMode};
use pgdt_dump::cache::{CacheMode, Held, Opened, SourceWatch, StrictIdentity};
use pgdt_io::{ByteRangeSource, Origin};
use pgdt_model::{Diagnostic, DumpIndex, PostgresInvalidValues, TableName};

use crate::budget::{Hold, ScanBudget};
use crate::error::Error;

/// How a dump is opened.
#[derive(Debug, Clone, Default)]
pub struct PgDumpOptions {
    /// The cache to read, where it is not the one `pgdt parse` writes by
    /// default — beside a local dump, or in the working directory named after
    /// a URL's last segment ([`CacheMode::resolve`]). A path is read as given:
    /// the provider always reads a cache.
    pub cache_path: Option<PathBuf>,
    /// Typed columns, or every column as text — the escape hatch from a wrong
    /// type mapping, per dump.
    pub schema_mode: SchemaMode,
    /// How a value PostgreSQL accepts for a column's declared type and the
    /// column's Arrow type cannot hold is read, the calendar DataFusion
    /// displays a `date` or timestamp through included: as NULL, the default,
    /// its column read as its text, or refused (`docs/design/decisions.md`,
    /// "D98", "D99"). Moot under [`SchemaMode::Strings`].
    pub unrepresentable: UnrepresentableMode,
    /// How a scan reads a field its type's `*_in` refuses, which the `pgdt
    /// parse` that built the cache went past: refused, by default, or read as
    /// the library's decoders read it, as
    /// [`QueryOptions::postgres_invalid_values`](pgdt_query::QueryOptions::postgres_invalid_values)
    /// says (`docs/design/decisions.md`, "D103"); `Strict`, a parse's
    /// mode, reads as the default does.
    pub postgres_invalid_values: PostgresInvalidValues,
    /// Which identity signals bind, as `pgdt --strict-identity` states them:
    /// between runs, what the cache is checked against at open; during one,
    /// whether a file changing under a scan fails it, and so whether a source
    /// nothing can check during a scan opens at all. The default is
    /// [`StrictIdentity::ADVISORY`], and [`StrictIdentity::NONE`] the only
    /// way to read an unpinnable remote dump.
    pub strict_identity: StrictIdentity,
}

/// A dump and the complete map of it its cache holds, opened once and shared
/// by every catalog, table and scan registered over it.
///
/// **It never maps and never writes.** The map is the one `pgdt parse` left,
/// loaded whole at open and believed only where it reaches the end of the file
/// and its identity checks pass as they do for any load. A cache that cannot
/// be used — another file's, another build's, damaged, or contradicting the
/// file's compression — is the library's own refusal; one short of a complete
/// map of this file, or none named, is an error naming the `pgdt parse` that
/// would build it.
///
/// **The map is resident for as long as the dump is registered**, because
/// DataFusion asks for a table's schema and statistics while it plans, and
/// the cache decodes whole. *Rejected: a reload per scan*, which decodes it
/// for every table every query names and answers "is this cache complete" a
/// second time, later. So a cache rebuilt after open is not seen until the
/// dump is opened again: a changed file fails the identity check at the next
/// scan's end, and an unchanged file's rebuilt cache differs only in the
/// statistics a later `pgdt parse` gathered.
///
/// **Its statistics are billed to every [`ScanBudget`] it is registered or
/// scanned under**, once each, for as long as the dump is alive: the map is
/// resident whether or not a scan runs.
pub struct PgDump {
    origin: String,
    source: Arc<dyn ByteRangeSource>,
    watch: Arc<SourceWatch>,
    index: DumpIndex,
    tables: Vec<TableName>,
    schema_mode: SchemaMode,
    unrepresentable: UnrepresentableMode,
    postgres_invalid_values: PostgresInvalidValues,
    /// The `pgdt parse` that builds this dump's cache, which a refusal names.
    parse: String,
    statistics_bytes: u64,
    /// One per budget this dump's statistics are billed to.
    holds: Mutex<Vec<Hold>>,
}

impl std::fmt::Debug for PgDump {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgDump").field("origin", &self.origin).finish_non_exhaustive()
    }
}

impl PgDump {
    /// Open `location` — a path, or anything `pgdt --source` accepts — through
    /// its complete cache.
    pub async fn open(location: &str, options: PgDumpOptions) -> Result<Arc<Self>, Error> {
        #[cfg(feature = "http")]
        let origin = Origin::resolve(location)?;
        #[cfg(not(feature = "http"))]
        let origin = Origin::local(location);
        let mode = CacheMode::resolve(&origin, options.cache_path.as_deref())
            .with_strict_identity(options.strict_identity);
        let incomplete = |why: String| Error::CacheNotComplete {
            parse: parse_command(location, options.cache_path.as_deref()),
            why,
        };
        // An unusable cache is the scans' own refusal, word for word: this
        // provider never writes, so nothing here may overwrite one, and the
        // remedy it names is `pgdt`'s (`docs/design/decisions.md`, "D20").
        let Opened { source, watch, cache } = mode.open(&origin).await?;
        let shown = || mode.path().map(Path::display).expect("a resolved cache has a path");
        let index = match cache {
            Held::Loaded(loaded) if loaded.is_complete() => loaded.index,
            Held::Loaded(loaded) => {
                return Err(incomplete(format!(
                    "the cache at {} covers {} of {} byte(s)",
                    shown(),
                    loaded.index.scanned_through,
                    loaded.total_size
                )));
            }
            Held::Missing => {
                return Err(incomplete(format!("there is no cache at {}", shown())));
            }
            Held::Disabled | Held::Replacing(_) => {
                unreachable!("the provider opens a cache it names and never replaces")
            }
        };
        let watch = Arc::new(watch);
        let tables = index.tables();
        let statistics_bytes = index.statistics_heap_bytes();
        Ok(Arc::new(Self {
            origin: origin.to_string(),
            source,
            watch,
            index,
            tables,
            schema_mode: options.schema_mode,
            unrepresentable: options.unrepresentable,
            postgres_invalid_values: options.postgres_invalid_values,
            parse: parse_command(location, options.cache_path.as_deref()),
            statistics_bytes,
            holds: Mutex::default(),
        }))
    }

    /// The dump as a message names it.
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// Every table the dump holds rows for, in the order its first block
    /// appears.
    pub fn tables(&self) -> &[TableName] {
        &self.tables
    }

    /// The databases the file holds: each `\connect`ed database by name, in
    /// file order, then `None` for one no `\connect` names — a plain
    /// single-database dump taken without `--create` — where it holds a
    /// table, one holding none yielding no entry. The stretch of a
    /// `pg_dumpall` file before its first `\connect`, which holds roles and
    /// no table, is not one.
    pub fn databases(&self) -> Vec<Option<String>> {
        let mut names: Vec<Option<String>> = Vec::new();
        let declared = self
            .index
            .metadata
            .iter()
            .flat_map(|m| &m.databases)
            .map(|db| &db.name)
            .filter(|name| name.is_some());
        let holding = self.tables.iter().map(|t| &t.database);
        for name in declared.chain(holding) {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
        names
    }

    /// The heap the statistics in this dump's resident map hold — what it
    /// bills each [`ScanBudget`] it is registered or scanned under.
    pub fn statistics_bytes(&self) -> u64 {
        self.statistics_bytes
    }

    /// Bill this dump's statistics to `budget`, unless they already are.
    pub(crate) fn bill(&self, budget: &Arc<ScanBudget>) {
        let mut holds = self.holds.lock().unwrap();
        if !holds.iter().any(|hold| hold.bills(budget)) {
            holds.push(budget.hold(self.statistics_bytes));
        }
    }

    /// What the file-level channel says about this dump and its cache.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.index.diagnostics
    }

    pub(crate) fn index(&self) -> &DumpIndex {
        &self.index
    }

    pub(crate) fn source(&self) -> &Arc<dyn ByteRangeSource> {
        &self.source
    }

    pub(crate) fn watch(&self) -> &Arc<SourceWatch> {
        &self.watch
    }

    pub(crate) fn schema_mode(&self) -> SchemaMode {
        self.schema_mode
    }

    pub(crate) fn unrepresentable(&self) -> UnrepresentableMode {
        self.unrepresentable
    }

    pub(crate) fn postgres_invalid_values(&self) -> PostgresInvalidValues {
        self.postgres_invalid_values
    }

    /// The `pgdt parse` that puts `table` at the data level, keeping every
    /// other table's level: a parse never records less than a cache holds.
    pub(crate) fn parse_at_data_level(&self, table: &TableName) -> String {
        format!("{} --statistics-level metadata,{}=data", self.parse, table.qualified_name())
    }
}

/// The `pgdt parse` that builds the cache this open looked for.
// deficiency: KD92 — it carries no `--postgres-invalid-values`, so one
// opened to ignore what PostgreSQL refuses names a parse that fails at the
// first such field it keys, where `ignore` would build the cache.
fn parse_command(location: &str, cache_path: Option<&Path>) -> String {
    match cache_path {
        Some(path) => format!("pgdt parse --source {location} --dtcache {}", path.display()),
        None => format!("pgdt parse --source {location}"),
    }
}
