//! What this binary adds to `datafusion-cli`: `--dump`, `STORED AS PGDUMP`,
//! and the sink printing what a registration finds to stderr. It never
//! parses: a dump without a complete cache is refused naming the `pgdt parse`
//! that builds one (`docs/design/decisions.md`, "D90").

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use crate::namespace_init::InitShutdown;
use async_trait::async_trait;
use datafusion::catalog::{Session, TableProvider, TableProviderFactory};
use datafusion::error::{DataFusionError, Result};
use datafusion::logical_expr::CreateExternalTable;
use datafusion::prelude::SessionContext;
use datafusion_pgdump::{
    PGDUMP_FILE_TYPE, PgDump, PgDumpOptions, PgDumpTableFactory, register_dump,
    register_table_factory,
};
use pgdump_query::{
    ComparisonDivergence, ComparisonNote, DiagnosticSink, Finding, SchemaMode, Severity,
};

/// As its PID namespace's init, end on every signal that ends this binary
/// anywhere else, exiting `128 + n` (`docs/design/decisions.md`, "D26") —
/// but `SIGINT` in the REPL, whose `ctrl_c` cancels a statement rather than
/// the session, and which every action registered on it would run beside.
pub fn end_as_namespace_init(repl: bool) -> Result<()> {
    let caught: &[libc::c_int] = if repl { &[libc::SIGINT] } else { &[] };
    InitShutdown::install(caught)?;
    Ok(())
}

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
///
/// **A column whose collation the dump does not record is counted, not
/// printed** ([`ComparisonDivergence::UnknownCollation`]): on a real dump that
/// is every text column of every table, one line each. [`StderrSink::fold`]
/// ends a registration by printing how many such columns in how many tables it
/// reported, in one line carrying the library's own reason for them; every
/// other finding prints as it arrives.
#[derive(Debug)]
struct StderrSink {
    quiet: bool,
    uncollated: Mutex<Uncollated>,
}

/// The uncollated columns one registration has reported so far.
#[derive(Debug, Default)]
struct Uncollated {
    /// `(table, column)`: a nested column reporting several positions is one
    /// column.
    columns: BTreeSet<(String, String)>,
    /// What the library's note says after naming the column.
    reason: Option<String>,
}

impl StderrSink {
    fn new(quiet: bool) -> Self {
        Self { quiet, uncollated: Mutex::default() }
    }

    /// End one registration, named `subject` — the dump as `--dump` gave it,
    /// or the table a `STORED AS PGDUMP` statement names: print what it
    /// reported of [`ComparisonDivergence::UnknownCollation`] as one line, if
    /// it reported any.
    fn fold(&self, subject: &str) {
        if let Some(line) = self.folded(subject) {
            eprintln!("{line}");
        }
    }

    /// The line [`StderrSink::fold`] prints, the count starting again.
    fn folded(&self, subject: &str) -> Option<String> {
        let Uncollated { columns, reason } = std::mem::take(&mut *self.uncollated.lock().unwrap());
        let tables = columns.iter().map(|(table, _)| table).collect::<BTreeSet<_>>().len();
        Some(format!(
            "warning: {subject}: {} column(s) in {tables} table(s) are each compared bytewise: \
             {}",
            columns.len(),
            reason?
        ))
    }

    /// Count `finding` if it is an uncollated column's note: the table is
    /// what the provider named in front of the note's own sentence.
    fn uncollated(&self, finding: &dyn Finding) -> bool {
        let Some(note) = finding.as_any().downcast_ref::<ComparisonNote>() else { return false };
        if note.divergence != ComparisonDivergence::UnknownCollation {
            return false;
        }
        let own = note.message();
        let located = finding.message();
        let table = located.strip_suffix(own.as_str()).and_then(|t| t.strip_suffix(": "));
        let reason = own.rsplit_once(" is compared bytewise: ").map_or(own.as_str(), |(_, r)| r);
        let mut uncollated = self.uncollated.lock().unwrap();
        uncollated.columns.insert((table.unwrap_or_default().to_string(), note.column.clone()));
        uncollated.reason.get_or_insert_with(|| reason.to_string());
        true
    }
}

impl DiagnosticSink for StderrSink {
    fn report(&self, finding: &dyn Finding) {
        let label = match finding.severity() {
            Severity::Info => return,
            Severity::Warning if self.quiet => return,
            Severity::Warning if self.uncollated(finding) => return,
            Severity::Warning => "warning",
            Severity::Error => "error",
        };
        eprintln!("{label}: {}", finding.message());
    }
}

/// The provider's `STORED AS PGDUMP`, with each statement's registration
/// ended by [`StderrSink::fold`], under the table name it gives.
#[derive(Debug)]
struct FoldingFactory {
    inner: PgDumpTableFactory,
    sink: Arc<StderrSink>,
}

#[async_trait]
impl TableProviderFactory for FoldingFactory {
    async fn create(
        &self,
        state: &dyn Session,
        cmd: &CreateExternalTable,
    ) -> Result<Arc<dyn TableProvider>> {
        let created = self.inner.create(state, cmd).await;
        self.sink.fold(&cmd.name.to_string());
        created
    }
}

/// Make `STORED AS PGDUMP` available in `ctx`, then open and register each
/// of `dumps`; a dump that cannot be opened or named ends the run, before any
/// SQL is read.
pub async fn register(ctx: &SessionContext, dumps: &[DumpArg], quiet: bool) -> Result<()> {
    let stderr = Arc::new(StderrSink::new(quiet));
    let sink: Arc<dyn DiagnosticSink> = Arc::clone(&stderr) as _;
    register_table_factory(ctx, Arc::clone(&sink));
    // The provider's factory, replaced by the same one ending each statement.
    let factory = FoldingFactory {
        inner: PgDumpTableFactory::new(Arc::clone(&sink)),
        sink: Arc::clone(&stderr),
    };
    ctx.state_ref()
        .write()
        .table_factories_mut()
        .insert(PGDUMP_FILE_TYPE.to_string(), Arc::new(factory));
    for dump in dumps {
        let options = PgDumpOptions { schema_mode: dump.schema_mode, ..PgDumpOptions::default() };
        let opened = PgDump::open(&dump.source, options).await.map_err(external)?;
        register_dump(ctx, dump.name.as_deref(), &opened, Arc::clone(&sink)).map_err(external)?;
        stderr.fold(opened.origin());
    }
    Ok(())
}

fn external(err: datafusion_pgdump::Error) -> DataFusionError {
    DataFusionError::External(Box::new(err))
}

#[cfg(test)]
mod tests {
    use std::any::Any;

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

    /// A finding named as the provider names one: its subject, then its own
    /// sentence, and the library's record behind [`Finding::as_any`].
    #[derive(Debug)]
    struct Named(&'static str, ComparisonNote);

    impl Finding for Named {
        fn severity(&self) -> Severity {
            self.1.severity()
        }

        fn message(&self) -> String {
            format!("{}: {}", self.0, self.1.message())
        }

        fn as_any(&self) -> &dyn Any {
            &self.1
        }
    }

    fn note(column: &str, path: Option<&str>, divergence: ComparisonDivergence) -> ComparisonNote {
        ComparisonNote {
            column: column.to_string(),
            path: path.map(str::to_string),
            declared_type: "text".to_string(),
            divergence,
        }
    }

    /// **One line per registration, counting columns and tables**: a nested
    /// column's two positions are one column, and the reason is the
    /// library's own sentence after the column it names. Any other
    /// divergence is not counted, and the count starts again.
    #[test]
    fn a_registration_s_uncollated_columns_fold_into_one_line() {
        use ComparisonDivergence::{LabelText, UnknownCollation};
        let sink = StderrSink::new(false);
        let first = note("name", None, UnknownCollation);
        for finding in [
            Named("shop.public.a", first.clone()),
            Named("shop.public.a", note("tags", Some("[]"), UnknownCollation)),
            Named("shop.public.a", note("tags", Some(".label"), UnknownCollation)),
            Named("shop.public.b", note("name", None, UnknownCollation)),
        ] {
            assert!(sink.uncollated(&finding), "{finding:?}");
        }
        assert!(!sink.uncollated(&Named("shop.public.b", note("mood", None, LabelText))));

        let sentence = first.message();
        let (_, reason) = sentence.split_once(" is compared bytewise: ").unwrap();
        assert_eq!(
            sink.folded("shop.sql").unwrap(),
            format!(
                "warning: shop.sql: 3 column(s) in 2 table(s) are each compared bytewise: {reason}"
            )
        );
        assert_eq!(sink.folded("shop.sql"), None);
    }

    /// **Under `--quiet` nothing is counted**, the folded line being a
    /// warning like those it replaces.
    #[test]
    fn a_quiet_sink_folds_nothing() {
        let sink = StderrSink::new(true);
        sink.report(&Named("t", note("name", None, ComparisonDivergence::UnknownCollation)));
        assert_eq!(sink.folded("t"), None);
    }
}
