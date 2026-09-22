//! The `pgdump.` session settings: what `pgdt query` takes as `--memory`,
//! `--chunk-size` and `--max-line-bytes`, set with `SET` and read when a scan
//! is planned (`docs/design/roadmap-P6-datafusion.md`, "Workers and memory").
//!
//! ```sql
//! SET pgdump.memory = 4294967296;
//! SET pgdump.chunk_size = 4194304;
//! ```

use std::any::Any;

use datafusion::catalog::Session;
use datafusion::common::config::{ConfigEntry, ConfigExtension, ExtensionOptions};
use datafusion::common::{Result, plan_err};
use datafusion::prelude::SessionContext;
use pgdump_query::{
    Parallelism, SCAN_CHUNK_DEFAULT_SIZE_BYTES, SCAN_LINE_DEFAULT_MAX_BYTES, ScanOptions,
};

/// The scan settings one DataFusion session states, under `pgdump.` in its
/// [`datafusion::common::config::ConfigOptions`], so `SET` moves them and `SHOW
/// ALL` lists them beside DataFusion's own.
///
/// **Read when a scan is planned**, so one set mid-session binds every scan
/// planned after it and none planned before: a live scan keeps what it drew,
/// as it keeps its partitions. A session that never registered them — a
/// provider registered by hand — plans under the defaults below.
///
/// **Each is a byte count, and none may be zero**, for the reasons `pgdt`'s
/// flags refuse one: no allowance a process can run in, a read that asks for
/// nothing forever, a limit refusing every line a read splits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PgDumpSettings {
    /// `pgdump.memory`: the resident allowance every pgdump scan of the
    /// session shares, where it is stated — `pgdt --memory`'s meaning, carved
    /// as [`crate::ScanBudget`] carves the one it discovered, which it
    /// overrides. Unstated, the budget's own.
    pub memory: Option<u64>,
    /// `pgdump.chunk_size`: bytes asked of the source per read
    /// ([`ScanOptions::chunk_size_bytes`]).
    pub chunk_size: usize,
    /// `pgdump.max_line_bytes`: the longest line a scan buffers before
    /// refusing the dump ([`ScanOptions::max_line_bytes`]).
    pub max_line_bytes: usize,
}

impl Default for PgDumpSettings {
    fn default() -> Self {
        Self {
            memory: None,
            chunk_size: SCAN_CHUNK_DEFAULT_SIZE_BYTES,
            max_line_bytes: SCAN_LINE_DEFAULT_MAX_BYTES,
        }
    }
}

impl PgDumpSettings {
    /// What `state` states, or the defaults where it carries none.
    pub(crate) fn of(state: &dyn Session) -> Self {
        state.config().options().extensions.get::<Self>().cloned().unwrap_or_default()
    }

    /// The [`ScanOptions`] a scan planned under these settings reads with.
    pub(crate) fn scan_options(&self, parallelism: Parallelism) -> ScanOptions {
        ScanOptions {
            chunk_size_bytes: self.chunk_size,
            max_line_bytes: self.max_line_bytes,
            parallelism,
            ..ScanOptions::default()
        }
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

/// A byte count `key` may be set to: a whole number, never zero, with `zero`
/// saying why not.
fn bytes(key: &str, value: &str, zero: &str) -> Result<u64> {
    match value.trim().parse::<u64>() {
        Ok(0) => plan_err!("pgdump.{key} cannot be 0: {zero}"),
        Ok(n) => Ok(n),
        Err(_) => plan_err!("pgdump.{key} is a whole number of bytes, not `{value}`"),
    }
}

/// A byte count as the `usize` a [`ScanOptions`] field holds.
fn usize_bytes(key: &str, value: &str, zero: &str) -> Result<usize> {
    let n = bytes(key, value, zero)?;
    match usize::try_from(n) {
        Ok(n) => Ok(n),
        Err(_) => plan_err!("pgdump.{key} of {n} bytes is more than this platform can address"),
    }
}

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
    /// what `SET` reaches; a whole key is taken too.
    fn set(&mut self, key: &str, value: &str) -> Result<()> {
        let field = key.strip_prefix("pgdump.").unwrap_or(key);
        match field {
            "memory" => {
                self.memory = Some(bytes(field, value, "that leaves no room to run in")?);
            }
            "chunk_size" => {
                self.chunk_size = usize_bytes(field, value, "a read of 0 bytes reads nothing")?;
            }
            "max_line_bytes" => {
                self.max_line_bytes =
                    usize_bytes(field, value, "it would refuse every line a read splits")?;
            }
            _ => {
                return plan_err!(
                    "`pgdump.{field}` is not a pgdump setting — the settings are pgdump.memory, \
                     pgdump.chunk_size and pgdump.max_line_bytes"
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
                 unset, the memory limit found, else half of what is available.",
            ),
            entry(
                "chunk_size",
                Some(self.chunk_size.to_string()),
                "Bytes a pgdump scan asks of the dump per read.",
            ),
            entry(
                "max_line_bytes",
                Some(self.max_line_bytes.to_string()),
                "The longest line, in bytes, a pgdump scan reads before refusing the dump.",
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::common::config::ConfigOptions;

    /// **`SET` reaches each setting by its key, and a zero, a non-number or an
    /// unknown key is refused**, leaving what was set before.
    #[test]
    fn each_setting_takes_a_nonzero_byte_count() {
        let mut options = ConfigOptions::new();
        options.extensions.insert(PgDumpSettings::default());
        options.set("pgdump.memory", "1073741824").unwrap();
        options.set("pgdump.chunk_size", "65536").unwrap();
        options.set("pgdump.max_line_bytes", "4096").unwrap();
        let stated =
            PgDumpSettings { memory: Some(1 << 30), chunk_size: 65536, max_line_bytes: 4096 };
        assert_eq!(options.extensions.get::<PgDumpSettings>(), Some(&stated));

        for (key, value) in [
            ("pgdump.memory", "0"),
            ("pgdump.chunk_size", "0"),
            ("pgdump.max_line_bytes", "0"),
            ("pgdump.memory", "4G"),
            ("pgdump.chunk_size", "-1"),
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
            ["pgdump.memory=1073741824", "pgdump.chunk_size=65536", "pgdump.max_line_bytes=4096"]
        );
    }

    /// Unstated, the allowance is the budget's own and the reads are the
    /// library's defaults.
    #[test]
    fn the_defaults_are_the_library_s() {
        let options =
            PgDumpSettings::default().scan_options(Parallelism::Serial { memory_bytes: None });
        let library = ScanOptions::default();
        assert_eq!(options.chunk_size_bytes, library.chunk_size_bytes);
        assert_eq!(options.max_line_bytes, library.max_line_bytes);
        assert_eq!(PgDumpSettings::default().memory, None);
    }
}
