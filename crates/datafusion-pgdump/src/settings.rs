//! The `pgdump.` session settings: what `pgdt query` takes as `--memory` and
//! `--chunk-size`, and whether a scan evaluates a dynamic filter's rows, set
//! with `SET` and read when a scan is planned.
//!
//! ```sql
//! SET pgdump.memory = 4294967296;
//! SET pgdump.chunk_size = 4194304;
//! SET pgdump.memory = 0;  -- the budget's own allowance again
//! SET pgdump.dynamic_filter_rows = true;
//! ```

use std::any::Any;

use datafusion::catalog::Session;
use datafusion::common::config::{ConfigEntry, ConfigExtension, ExtensionOptions};
use datafusion::common::{Result, plan_err};
use datafusion::prelude::SessionContext;
use pgdt_dump::ReadOptions;
use pgdt_io::SCAN_CHUNK_DEFAULT_SIZE_BYTES;
use pgdt_query::RowEvaluation;

/// The scan settings one DataFusion session states, under `pgdump.` in its
/// [`datafusion::common::config::ConfigOptions`], so `SET` moves them and `SHOW
/// ALL` lists them beside DataFusion's own.
///
/// **Read when a scan is planned**, so one set mid-session binds every scan
/// planned after it and none planned before: a live scan keeps what it drew,
/// as it keeps its partitions. A session neither readied
/// ([`ready_session`](crate::ready_session)) nor registered into by this crate
/// — a provider registered by hand in it — plans under the defaults below.
///
/// **Each but `pgdump.dynamic_filter_rows` is a byte count.** A read chunk of
/// zero is refused, as `pgdt`'s flag refuses one: a read that asks for
/// nothing forever.
///
/// **`pgdump.memory = 0` un-states the allowance**, returning the session to
/// its budget's own — the discovered one, where the provider installed the
/// budget — as `datafusion.execution.target_partitions = 0` returns to the
/// machine's parallelism: a `SET` cannot be taken back, and
/// DataFusion 55's `RESET` reaches only its own `datafusion.` keys (UF3). `pgdt
/// --memory 0` stays refused, because a flag is un-stated by leaving it off —
/// absence is how `pgdt` asks for the discovered allowance
/// (`docs/design/decisions.md`, "D64") — so there `0` could only mean no room to
/// run in.
///
/// *Rejected: an allowance fixed at startup*, which a REPL user could change
/// only by restarting; *per-table `OPTIONS`*, a read chunk being the device's,
/// which one `SET` covers;
/// *a sentinel word for "discovered"*, which no DataFusion setting spells; and
/// *a flag per setting on `pgdt sql`*, another line of upstream's
/// `main.rs` to carry, where `-c` and `--rc` already run a `SET` at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PgDumpSettings {
    /// `pgdump.memory`: the resident allowance every pgdump scan of the
    /// session shares, where it is stated — `pgdt --memory`'s meaning, carved
    /// as [`crate::ScanBudget`] carves the one it discovered, which it
    /// overrides. Unstated, or set to `0`, the budget's own.
    pub memory: Option<u64>,
    /// `pgdump.chunk_size`: bytes asked of the source per read
    /// ([`ReadOptions::chunk_size_bytes`]).
    pub chunk_size: usize,
    /// `pgdump.dynamic_filter_rows`: whether a scan drops each row a join's,
    /// a TopK's or an aggregate's dynamic filter rejects before decoding it,
    /// a boolean ([`RowEvaluation`]). Off by default
    /// (`docs/design/decisions.md`, "D93"); no `pgdt` flag answers it,
    /// `pgdt query` holding no dynamic filter.
    pub dynamic_filter_rows: RowEvaluation,
}

impl Default for PgDumpSettings {
    fn default() -> Self {
        Self {
            memory: None,
            chunk_size: SCAN_CHUNK_DEFAULT_SIZE_BYTES,
            dynamic_filter_rows: RowEvaluation::default(),
        }
    }
}

impl PgDumpSettings {
    /// What `state` states, or the defaults where it carries none.
    pub(crate) fn of(state: &dyn Session) -> Self {
        state.config().options().extensions.get::<Self>().cloned().unwrap_or_default()
    }

    /// The [`ReadOptions`] a scan planned under these settings reads with.
    pub(crate) fn read_options(&self) -> ReadOptions {
        ReadOptions::default().with_chunk_size(self.chunk_size)
    }

    /// Give `ctx` these settings at their defaults, unless it already carries
    /// some, whose stated values stand.
    pub(crate) fn install(ctx: &SessionContext) {
        let state = ctx.state_ref();
        let mut state = state.write();
        let options = state.config_mut().options_mut();
        if options.extensions.get::<Self>().is_none() {
            options.extensions.insert(Self::default());
        }
    }
}

impl ConfigExtension for PgDumpSettings {
    const PREFIX: &'static str = "pgdump";
}

/// Every setting's key, without its `pgdump.`.
const SETTINGS: [&str; 3] = ["memory", "chunk_size", "dynamic_filter_rows"];

/// Every key `CREATE EXTERNAL TABLE … STORED AS PGDUMP` takes, without its
/// `pgdump.`: listed here, below the factory reading them, so `SET` can refuse
/// one as that statement's option rather than as no key at all.
pub(crate) const TABLE_OPTIONS: [&str; 8] = [
    "table",
    "schema",
    "database",
    "schema_mode",
    "unrepresentable",
    "postgres_invalid_values",
    "inheritance",
    "strict_identity",
];

/// `keys` with their `pgdump.`, as a refusal lists them.
pub(crate) fn listed(keys: &[&str]) -> String {
    let keys: Vec<String> = keys.iter().map(|key| format!("pgdump.{key}")).collect();
    match keys.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        _ => keys.concat(),
    }
}

/// A whole number of bytes `key` may be set to, zero included.
fn whole(key: &str, value: &str) -> Result<u64> {
    match value.trim().parse::<u64>() {
        Ok(n) => Ok(n),
        Err(_) => plan_err!("pgdump.{key} is a whole number of bytes, not `{value}`"),
    }
}

/// A byte count `key` may be set to: a whole number, never zero, with `zero`
/// saying why not.
fn bytes(key: &str, value: &str, zero: &str) -> Result<u64> {
    match whole(key, value)? {
        0 => plan_err!("pgdump.{key} cannot be 0: {zero}"),
        n => Ok(n),
    }
}

/// Whether `key` is on, spelled as DataFusion spells a boolean setting.
fn boolean(key: &str, value: &str) -> Result<bool> {
    match value.trim().to_lowercase().parse::<bool>() {
        Ok(on) => Ok(on),
        Err(_) => plan_err!("pgdump.{key} is true or false, not `{value}`"),
    }
}

/// A byte count as the `usize` a [`ReadOptions`] field holds.
fn usize_bytes(key: &str, value: &str, zero: &str) -> Result<usize> {
    let n = bytes(key, value, zero)?;
    match usize::try_from(n) {
        Ok(n) => Ok(n),
        Err(_) => plan_err!("pgdump.{key} of {n} bytes is more than this platform can address"),
    }
}

// upstream: UF3
impl ExtensionOptions for PgDumpSettings {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn cloned(&self) -> Box<dyn ExtensionOptions> {
        Box::new(self.clone())
    }

    /// `key` arrives without its `pgdump.` from `ConfigOptions::set`, which is
    /// what `SET` reaches; a whole key is taken too. A key `STORED AS PGDUMP`
    /// takes is refused as that statement's option, read where a table is
    /// registered (`docs/design/decisions.md`, "D132").
    fn set(&mut self, key: &str, value: &str) -> Result<()> {
        let field = key.strip_prefix("pgdump.").unwrap_or(key);
        match field {
            "memory" => {
                self.memory = Some(whole(field, value)?).filter(|&n| n > 0);
            }
            "chunk_size" => {
                self.chunk_size = usize_bytes(field, value, "a read of 0 bytes reads nothing")?;
            }
            "dynamic_filter_rows" => {
                self.dynamic_filter_rows = match boolean(field, value)? {
                    true => RowEvaluation::On,
                    false => RowEvaluation::Off,
                };
            }
            _ if TABLE_OPTIONS.contains(&field) => {
                return plan_err!(
                    "`pgdump.{field}` is a `STORED AS PGDUMP` option, not a session setting: it is \
                     stated where the table is registered, as `OPTIONS ('pgdump.{field}' …)`, and \
                     `SET` reaches only {}",
                    listed(&SETTINGS)
                );
            }
            _ => {
                return plan_err!(
                    "`pgdump.{field}` is not a pgdump setting — the settings are {}",
                    listed(&SETTINGS)
                );
            }
        }
        Ok(())
    }

    fn entries(&self) -> Vec<ConfigEntry> {
        let entry = |key: &str, value: Option<String>, description| ConfigEntry {
            key: format!("pgdump.{key}"),
            value,
            description,
        };
        vec![
            entry(
                "memory",
                self.memory.map(|n| n.to_string()),
                "Bytes every pgdump scan of the session may hold resident between them; \
                 unset or 0, the allowance the session's pgdump budget was built with — \
                 unless its embedder stated one, the memory limit found, else half of what \
                 is available, else the library's default, shared between the scans.",
            ),
            entry(
                "chunk_size",
                Some(self.chunk_size.to_string()),
                "Bytes a pgdump scan asks of the dump per read.",
            ),
            entry(
                "dynamic_filter_rows",
                Some((self.dynamic_filter_rows == RowEvaluation::On).to_string()),
                "Whether a pgdump scan drops each row a join's, a TopK's or an aggregate's \
                 dynamic filter rejects before decoding it. Off, the filter still skips the row \
                 groups it rules out and stops a sorted block, and its producer drops the rows.",
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::common::config::ConfigOptions;

    /// **`SET` reaches each setting by its key, and a zero chunk, a
    /// non-number, a boolean spelled otherwise than DataFusion's or
    /// an unknown key is refused**, leaving what was set before; a zero
    /// allowance un-states the one set.
    #[test]
    fn each_setting_takes_a_byte_count() {
        let mut options = ConfigOptions::new();
        options.extensions.insert(PgDumpSettings::default());
        options.set("pgdump.memory", "1073741824").unwrap();
        options.set("pgdump.chunk_size", "65536").unwrap();
        options.set("pgdump.dynamic_filter_rows", "TRUE").unwrap();
        let stated = PgDumpSettings {
            memory: Some(1 << 30),
            chunk_size: 65536,
            dynamic_filter_rows: RowEvaluation::On,
        };
        assert_eq!(options.extensions.get::<PgDumpSettings>(), Some(&stated));

        for (key, value) in [
            ("pgdump.chunk_size", "0"),
            ("pgdump.max_line_bytes", "1073741822"),
            ("pgdump.memory", "4G"),
            ("pgdump.chunk_size", "-1"),
            ("pgdump.dynamic_filter_rows", "1"),
            ("pgdump.jobs", "4"),
        ] {
            assert!(options.set(key, value).is_err(), "{key} = {value}");
        }
        assert_eq!(options.extensions.get::<PgDumpSettings>(), Some(&stated));

        let listed: Vec<String> = options
            .entries()
            .into_iter()
            .filter(|entry| entry.key.starts_with("pgdump."))
            .map(|entry| format!("{}={}", entry.key, entry.value.unwrap_or_default()))
            .collect();
        assert_eq!(
            listed,
            [
                "pgdump.memory=1073741824",
                "pgdump.chunk_size=65536",
                "pgdump.dynamic_filter_rows=true",
            ]
        );

        options.set("pgdump.memory", "0").unwrap();
        let unstated = PgDumpSettings { memory: None, ..stated };
        assert_eq!(options.extensions.get::<PgDumpSettings>(), Some(&unstated));
        assert!(options.entries().iter().any(|e| e.key == "pgdump.memory" && e.value.is_none()));
    }

    /// **A key `STORED AS PGDUMP` takes is refused as that statement's
    /// option**, naming where it is stated, and any other key as no setting,
    /// each refusal listing every setting `entries` lists.
    #[test]
    fn a_table_option_is_refused_as_one() {
        let mut options = ConfigOptions::new();
        options.extensions.insert(PgDumpSettings::default());
        let settings = listed(&SETTINGS);
        for field in TABLE_OPTIONS {
            let why = options.set(&format!("pgdump.{field}"), "only").unwrap_err().to_string();
            assert!(
                why.contains(&format!("`pgdump.{field}` is a `STORED AS PGDUMP` option")),
                "{why}"
            );
            assert!(why.contains(&format!("OPTIONS ('pgdump.{field}' …)")), "{why}");
            assert!(why.contains(&settings), "{why}");
        }
        let why = options.set("pgdump.jobs", "4").unwrap_err().to_string();
        assert!(why.contains("`pgdump.jobs` is not a pgdump setting"), "{why}");
        assert!(why.contains(&settings), "{why}");
        assert_eq!(options.extensions.get::<PgDumpSettings>(), Some(&PgDumpSettings::default()));

        let keys: Vec<String> =
            PgDumpSettings::default().entries().into_iter().map(|entry| entry.key).collect();
        assert_eq!(keys, SETTINGS.map(|key| format!("pgdump.{key}")));
    }

    /// Unstated, the allowance is the budget's own, the reads are the
    /// library's defaults and a dynamic filter's rows are not evaluated.
    #[test]
    fn the_defaults_are_the_library_s() {
        assert_eq!(PgDumpSettings::default().read_options(), ReadOptions::default());
        assert_eq!(PgDumpSettings::default().memory, None);
        assert_eq!(PgDumpSettings::default().dynamic_filter_rows, RowEvaluation::Off);
    }
}
