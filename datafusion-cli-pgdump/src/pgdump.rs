//! What this binary adds to `datafusion-cli`: `--dump`, `STORED AS PGDUMP`,
//! and the sink printing what a registration finds to stderr
//! (`docs/design/roadmap-P6-datafusion.md`, "The binary:
//! `datafusion-cli-pgdump`" and "Diagnostics: one sink").

use std::sync::Arc;

use datafusion::error::{DataFusionError, Result};
use datafusion::prelude::SessionContext;
use datafusion_pgdump::{PgDump, PgDumpOptions, register_dump, register_table_factory};
use pgdump_query::{DiagnosticSink, Finding, SchemaMode, Severity};

pub const DUMP_HELP: &str = "Register a pg_dump file as catalogs, one per database it holds, \
    read through the cache `pgdt parse` leaves. A database the file names is a catalog of that \
    name; NAME= names the catalog of a dump that names no database. :strings reads every column \
    as its text. Repeatable";

/// One `--dump [NAME=]SOURCE[:strings]`.
///
/// **`NAME=` is recognised only where what precedes the first `=` could not
/// be part of a path or a URL** — no `/`, `\`, `.` or `:` — so a URL's query
/// string is never read as a name; a local path holding `=` is written
/// `./a=b.sql`.
#[derive(Debug, Clone, PartialEq)]
pub struct DumpArg {
    pub name: Option<String>,
    pub source: String,
    pub schema_mode: SchemaMode,
}

impl DumpArg {
    pub fn parse(arg: &str) -> Result<Self, String> {
        let (arg, schema_mode) = match arg.strip_suffix(":strings") {
            Some(rest) => (rest, SchemaMode::Strings),
            None => (arg, SchemaMode::Typed),
        };
        let (name, source) = match arg.split_once('=') {
            Some((name, source)) if !name.is_empty() && !name.contains(['/', '\\', '.', ':']) => {
                (Some(name.to_string()), source)
            }
            _ => (None, arg),
        };
        if source.is_empty() {
            return Err("--dump names no source".to_string());
        }
        Ok(Self { name, source: source.to_string(), schema_mode })
    }
}

/// Prints a finding to stderr as `<severity>: <sentence>`, as `pgdt` prints
/// its own: a `Warning` or an `Error`, never an `Info` — every mapped column
/// earns one, which on a real dump is thousands of lines saying nothing is
/// wrong — and under `--quiet` an `Error` alone.
struct StderrSink {
    quiet: bool,
}

impl DiagnosticSink for StderrSink {
    fn report(&self, finding: &dyn Finding) {
        let label = match finding.severity() {
            Severity::Info => return,
            Severity::Warning if self.quiet => return,
            Severity::Warning => "warning",
            Severity::Error => "error",
        };
        eprintln!("{label}: {}", finding.message());
    }
}

/// Make `STORED AS PGDUMP` available in `ctx`, then open and register each
/// of `dumps`; a dump that cannot be opened or named ends the run, before any
/// SQL is read.
pub async fn register(ctx: &SessionContext, dumps: &[DumpArg], quiet: bool) -> Result<()> {
    let sink: Arc<dyn DiagnosticSink> = Arc::new(StderrSink { quiet });
    register_table_factory(ctx, Arc::clone(&sink));
    for dump in dumps {
        let options = PgDumpOptions { schema_mode: dump.schema_mode, ..PgDumpOptions::default() };
        let opened = PgDump::open(&dump.source, options).await.map_err(external)?;
        register_dump(ctx, dump.name.as_deref(), &opened, Arc::clone(&sink)).map_err(external)?;
    }
    Ok(())
}

fn external(err: datafusion_pgdump::Error) -> DataFusionError {
    DataFusionError::External(Box::new(err))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dump(name: Option<&str>, source: &str, schema_mode: SchemaMode) -> DumpArg {
        DumpArg { name: name.map(str::to_string), source: source.to_string(), schema_mode }
    }

    fn parsed(arg: &str) -> DumpArg {
        DumpArg::parse(arg).unwrap()
    }

    #[test]
    fn a_name_is_what_precedes_an_equals_that_no_path_could() {
        assert_eq!(parsed("koji.dump"), dump(None, "koji.dump", SchemaMode::Typed));
        assert_eq!(
            parsed("koji=/d/koji.dump"),
            dump(Some("koji"), "/d/koji.dump", SchemaMode::Typed)
        );
        assert_eq!(
            parsed("k=koji.dump.xz:strings"),
            dump(Some("k"), "koji.dump.xz", SchemaMode::Strings)
        );
        assert_eq!(parsed("/d/koji.dump:strings"), dump(None, "/d/koji.dump", SchemaMode::Strings));
        // A URL's query string is not a name, and neither is a path's `=`.
        let url = "https://h/koji.dump?sig=abc";
        assert_eq!(parsed(url), dump(None, url, SchemaMode::Typed));
        assert_eq!(parsed("./a=b.sql"), dump(None, "./a=b.sql", SchemaMode::Typed));
        assert_eq!(parsed("dir/a=b.sql"), dump(None, "dir/a=b.sql", SchemaMode::Typed));
        assert!(DumpArg::parse("koji=").is_err());
        assert!(DumpArg::parse(":strings").is_err());
    }
}
