//! A dump opened through its complete cache (`docs/design/decisions.md`,
//! "D90").

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use pgdt_codec::{SchemaMode, UnrepresentableMode};
use pgdt_dump::cache::{CacheMode, Held, StrictIdentity};
use pgdt_io::Origin;
use pgdt_model::{Diagnostic, PostgresInvalidValues, TableName};
use pgdt_query::Dump;

use crate::budget::{Hold, ScanBudget};
use crate::error::{Error, MappingPass, Speller, Spelling};

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
    /// The front end's words for the remedy each refusal names, which every
    /// refusal of this dump, its tables and their scans carries in its
    /// message ([`Spelling`]). `None`, the default, leaves each in the
    /// library's terms alone.
    pub spelling: Option<Arc<dyn Spelling>>,
}

/// A dump and the complete map of it its cache holds ([`Dump`]), opened once
/// and shared by every catalog, table and scan registered over it, with the
/// modes every scan of it reads under and the budgets its statistics are
/// billed to.
///
/// **It never maps and never writes.** The map is the one `pgdt parse` left,
/// loaded whole at open and believed only where it reaches the end of the file
/// and its identity checks pass as they do for any load. A cache that cannot
/// be used — another file's, another build's, damaged, or contradicting the
/// file's compression — is the library's own refusal; one short of a complete
/// map of this file, or none named, is [`Error::CacheNotComplete`], carrying
/// the mapping pass that would build it.
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
    dump: Dump,
    schema_mode: SchemaMode,
    unrepresentable: UnrepresentableMode,
    postgres_invalid_values: PostgresInvalidValues,
    /// The mapping pass that builds this dump's cache, which a refusal names.
    pass: MappingPass,
    /// The front end's spelling, which every refusal of this dump carries.
    speller: Speller,
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
    /// its complete cache, refused as [`PgDumpOptions::spelling`] spells it.
    pub async fn open(location: &str, options: PgDumpOptions) -> Result<Arc<Self>, Error> {
        let speller = Speller(options.spelling.clone());
        Self::open_unspelled(location, options, speller.clone())
            .await
            .map_err(|err| speller.error(err))
    }

    async fn open_unspelled(
        location: &str,
        options: PgDumpOptions,
        speller: Speller,
    ) -> Result<Arc<Self>, Error> {
        #[cfg(feature = "http")]
        let origin = Origin::resolve(location)?;
        #[cfg(not(feature = "http"))]
        let origin = Origin::local(location);
        let mode = CacheMode::resolve(&origin, options.cache_path.as_deref())
            .with_strict_identity(options.strict_identity);
        let pass = MappingPass {
            location: location.to_string(),
            cache_path: options.cache_path.clone(),
            postgres_invalid_values: options.postgres_invalid_values,
        };
        let incomplete = |why: String| Error::CacheNotComplete { pass: pass.clone(), why };
        // An unusable cache is the scans' own refusal, word for word: this
        // provider never writes, so nothing here may overwrite one
        // (`docs/design/decisions.md`, "D20").
        let opened = mode.open(&origin).await?;
        // Every scan reads, so a source its in-flight check cannot cover is
        // refused at registration rather than at a scan.
        opened.watch.require_checkable(opened.source.as_ref())?;
        let shown = || mode.path().map(Path::display).expect("a resolved cache has a path");
        match &opened.cache {
            Held::Loaded(loaded) if loaded.is_complete() => {}
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
        }
        let dump = Dump::opened(opened).await?;
        let statistics_bytes = dump.index().statistics_heap_bytes();
        Ok(Arc::new(Self {
            origin: origin.to_string(),
            dump,
            schema_mode: options.schema_mode,
            unrepresentable: options.unrepresentable,
            postgres_invalid_values: options.postgres_invalid_values,
            pass,
            speller,
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
        self.dump.tables()
    }

    /// The databases the file holds ([`Dump::databases`]).
    pub fn databases(&self) -> Vec<Option<String>> {
        self.dump.databases()
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
        &self.dump.index().diagnostics
    }

    /// The dump and its complete map, which a table of it is resolved from
    /// once ([`Dump::table`]).
    pub(crate) fn dump(&self) -> &Dump {
        &self.dump
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

    /// The mapping pass that builds this dump's cache, which a refusal names.
    pub(crate) fn pass(&self) -> &MappingPass {
        &self.pass
    }

    /// The front end's spelling, which every refusal of this dump carries.
    pub(crate) fn speller(&self) -> &Speller {
        &self.speller
    }
}
