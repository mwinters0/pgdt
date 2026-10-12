//! What this crate adds to `datafusion-cli`: `--dump`, `STORED AS PGDUMP`,
//! and the sink printing what a registration finds to stderr; ending as its
//! PID namespace's init (`docs/design/decisions.md`, "D26"); and what a caller
//! composing the CLI reads of it — the copied release's version, `--dump`'s
//! and `--strict-identity`'s help texts, and the introspection build's report
//! section. It never parses: a dump without a complete cache is refused
//! naming the `pgdt parse` that builds one (`docs/design/decisions.md`,
//! "D90").

use std::sync::Arc;

use datafusion::error::Result;
use datafusion::prelude::SessionContext;
use datafusion_pgdump::{PgDump, PgDumpOptions, register_dump, register_table_factory};
use namespace_init::InitShutdown;
use pgdt_codec::{SchemaMode, UnrepresentableMode};
use pgdt_dump::cache::StrictIdentity;
use pgdt_model::{DiagnosticSink, Finding, PostgresInvalidValues, Severity};

/// The `datafusion-cli` release the copy in `src/lib.rs` is, for a caller
/// naming it beside its own version.
pub const DATAFUSION_VERSION: &str = datafusion_cli::DATAFUSION_CLI_VERSION;

/// The environment variable naming the file the introspection build writes
/// its report to — `pgdt`'s, whose `sql` writes this section into its report.
/// Unset, nothing is written.
#[cfg(feature = "introspect")]
pub const INTROSPECT_OUT_VAR: &str = "PGDT_INTROSPECT_OUT";

/// The introspection build's section of a report: what
/// `pgdt_core::instrument` timed of each row a dynamic filter's state was
/// evaluated on, as `key=value` lines under the scope they cover. Read once
/// [`crate::run`] has returned, no thread then evaluating.
///
/// **A section, not a file**: the report is the process's, written once by
/// the binary that owns it — `pgdt sql`'s, beside the allocator's sections —
/// since a second writer to the one file would replace the first.
///
/// **An instrument, never timed**: `scripts/measure.py` times only the build
/// its own `cargo build --release` makes, which carries no feature, and
/// `measure.py --profile-recipe` builds this one into a target directory of
/// its own. Absent from that build.
#[cfg(feature = "introspect")]
pub fn introspection_section() -> String {
    let mut out = String::from("evaluation_scope=dynamic-filter-row-evaluation\n");
    out.push_str(&pgdt_core::instrument::evaluation_reading().lines());
    out
}

/// As its PID namespace's init, end on every signal but a fault that ends this
/// binary anywhere else, exiting `128 + n` (`docs/design/decisions.md`, "D26") —
/// but `SIGINT` in the REPL, whose `ctrl_c` cancels a statement rather than
/// the session, and which every action registered on it would run beside.
pub fn end_as_namespace_init(repl: bool) -> Result<()> {
    let caught: &[namespace_init::c_int] = if repl { &[namespace_init::SIGINT] } else { &[] };
    InitShutdown::install(caught)?;
    Ok(())
}

pub const DUMP_HELP: &str = "Register a pg_dump file as catalogs, one per database it holds, \
    read through the cache `pgdt parse` leaves. A database the file names is a catalog of that \
    name, unless NAME= is given; NAME= is required for a dump that names no database, and \
    refused for one of several. :strings reads every column as its text, \
    :unrepresentable=refuse refuses at planning a query needing the values of a column holding \
    one its type cannot hold, where the default, `null`, reads such a value as NULL and `text` \
    reads such a column as its text, :postgres-invalid-values=ignore reads what PostgreSQL \
    refuses and the default, `default`, refuses too where it can — a float past its type's \
    range as the largest value of its sign and one below it as zero, a `varchar(n)` or \
    `char(n)` value past its length as written, a NULL in a `NOT NULL` column as NULL — and \
    :strict-identity=TERMS states this dump's \
    strictness where --strict-identity would. Repeatable";

pub const STRICT_IDENTITY_HELP: &str = "Bind identity signals, as `pgdt --strict-identity` \
    does, for every --dump and STORED AS PGDUMP that states none of its own: `time` refuses a \
    cache whose recorded modification signal the dump no longer states, `location` one written \
    for another URL, the bare flag both. A dump changing under a scan fails it under every \
    selection but `none`, so a server stating neither a strong entity tag nor a Last-Modified \
    is refused; `none` binds nothing, that check included, and reads it anyway, and `advisory` is the default, which a dump under a stricter \
    session states to keep that check";

/// One `--dump [NAME=]SOURCE[:strings][:unrepresentable=MODE][:postgres-invalid-values=MODE][:strict-identity=TERMS]`.
///
/// **`NAME=` is recognised only where what precedes the first `=` could not
/// be part of a path or a URL** — no `/`, `\`, `.` or `:` — so a URL's query
/// string is never read as a name; a local path holding `=` is written
/// `./a=b.sql`. **The suffixes are stripped from the right, in any order**,
/// each at most once: TERMS hold `,` and never `:`, and MODE is `null`,
/// `text` or `refuse` for `:unrepresentable=`, `default` or `ignore` for
/// `:postgres-invalid-values=`.
#[derive(Debug, Clone, PartialEq)]
pub struct DumpArg {
    pub name: Option<String>,
    pub source: String,
    pub schema_mode: SchemaMode,
    /// How this dump reads a value its column's type cannot hold.
    pub unrepresentable: UnrepresentableMode,
    /// How this dump reads a value PostgreSQL refuses for its column's type.
    pub postgres_invalid_values: PostgresInvalidValues,
    /// This dump's strictness, where `--strict-identity` gives the default.
    pub strict_identity: Option<StrictIdentity>,
}

impl DumpArg {
    pub fn parse(arg: &str) -> Result<Self, String> {
        const STRICT: &str = ":strict-identity=";
        const UNREPRESENTABLE: &str = ":unrepresentable=";
        const INVALID: &str = ":postgres-invalid-values=";
        let mut arg = arg;
        let mut strings = false;
        let mut strict_identity = None;
        let mut unrepresentable = None;
        let mut invalid = None;
        loop {
            if let Some(rest) = arg.strip_suffix(":strings") {
                if std::mem::replace(&mut strings, true) {
                    return Err("--dump states :strings twice".to_string());
                }
                arg = rest;
            } else if let Some((rest, mode)) =
                arg.rsplit_once(UNREPRESENTABLE).filter(|(_, mode)| !mode.contains(':'))
            {
                let mode = match mode {
                    "null" => UnrepresentableMode::Null,
                    "text" => UnrepresentableMode::Text,
                    "refuse" => UnrepresentableMode::Refuse,
                    _ => {
                        return Err(format!(
                            "--dump {UNREPRESENTABLE}{mode}: the mode is `null`, `text` or `refuse`"
                        ));
                    }
                };
                if unrepresentable.replace(mode).is_some() {
                    return Err(format!("--dump states {UNREPRESENTABLE} twice"));
                }
                arg = rest;
            } else if let Some((rest, mode)) =
                arg.rsplit_once(INVALID).filter(|(_, mode)| !mode.contains(':'))
            {
                let mode = match mode {
                    "default" => PostgresInvalidValues::Default,
                    "ignore" => PostgresInvalidValues::Ignore,
                    _ => {
                        return Err(format!(
                            "--dump {INVALID}{mode}: the mode is `default` or `ignore`"
                        ));
                    }
                };
                if invalid.replace(mode).is_some() {
                    return Err(format!("--dump states {INVALID} twice"));
                }
                arg = rest;
            } else if let Some((rest, terms)) =
                arg.rsplit_once(STRICT).filter(|(_, terms)| !terms.contains(':'))
            {
                let strict =
                    terms.parse().map_err(|why| format!("--dump {STRICT}{terms}: {why}"))?;
                if strict_identity.replace(strict).is_some() {
                    return Err(format!("--dump states {STRICT} twice"));
                }
                arg = rest;
            } else {
                break;
            }
        }
        let schema_mode = if strings { SchemaMode::Strings } else { SchemaMode::Typed };
        let (name, source) = match arg.split_once('=') {
            Some((name, source)) if !name.is_empty() && !name.contains(['/', '\\', '.', ':']) => {
                (Some(name.to_string()), source)
            }
            _ => (None, arg),
        };
        if source.is_empty() {
            return Err("--dump names no source".to_string());
        }
        Ok(Self {
            name,
            source: source.to_string(),
            schema_mode,
            unrepresentable: unrepresentable.unwrap_or_default(),
            postgres_invalid_values: invalid.unwrap_or_default(),
            strict_identity,
        })
    }
}

/// Prints a finding to stderr as `<severity>: <sentence>`, as `pgdt` prints
/// its own: a `Warning` or an `Error`, never an `Info` — every mapped column
/// earns one, which on a real dump is thousands of lines saying nothing is
/// wrong — and under `--quiet` an `Error` alone.
#[derive(Debug)]
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
/// SQL is read. `strict_identity` is the session's, which a dump or a
/// statement stating its own overrides.
pub async fn register(
    ctx: &SessionContext,
    dumps: &[DumpArg],
    strict_identity: StrictIdentity,
    quiet: bool,
) -> Result<()> {
    let sink: Arc<dyn DiagnosticSink> = Arc::new(StderrSink { quiet });
    register_table_factory(ctx, Arc::clone(&sink), strict_identity);
    for dump in dumps {
        let options = PgDumpOptions {
            schema_mode: dump.schema_mode,
            unrepresentable: dump.unrepresentable,
            postgres_invalid_values: dump.postgres_invalid_values,
            strict_identity: dump.strict_identity.unwrap_or(strict_identity),
            ..PgDumpOptions::default()
        };
        let opened = PgDump::open(&dump.source, options).await?;
        register_dump(ctx, dump.name.as_deref(), &opened, Arc::clone(&sink))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dump(name: Option<&str>, source: &str, schema_mode: SchemaMode) -> DumpArg {
        DumpArg {
            name: name.map(str::to_string),
            source: source.to_string(),
            schema_mode,
            unrepresentable: UnrepresentableMode::Null,
            postgres_invalid_values: PostgresInvalidValues::Default,
            strict_identity: None,
        }
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

    /// **`:strict-identity=TERMS` is this dump's strictness**, in `pgdt`'s
    /// grammar, beside `:strings` in either order and each at most once; a
    /// `:` after it is the source's, not a term. `advisory` is the default
    /// stated, so a dump can loosen a stricter session's selection to it.
    #[test]
    fn a_dump_states_its_own_strictness_as_a_suffix() {
        let strict = |arg: &str| DumpArg::parse(arg).unwrap().strict_identity;
        assert_eq!(strict("koji.dump"), None);
        assert_eq!(strict("k=koji.dump:strict-identity=none"), Some(StrictIdentity::NONE));
        assert_eq!(strict("koji.dump:strict-identity=advisory"), Some(StrictIdentity::ADVISORY));
        let time = Some(StrictIdentity::binding(true, false));
        for arg in
            ["koji.dump:strings:strict-identity=time", "koji.dump:strict-identity=time:strings"]
        {
            let parsed = parsed(arg);
            assert_eq!(
                (parsed.source.as_str(), parsed.schema_mode, parsed.strict_identity),
                ("koji.dump", SchemaMode::Strings, time),
                "{arg}"
            );
        }
        assert_eq!(
            strict("https://h/koji.dump:strict-identity=time,location"),
            Some(StrictIdentity::binding(true, true))
        );
        let odd = parsed("d/a:strict-identity=time:b.sql");
        assert_eq!(
            (odd.source.as_str(), odd.strict_identity),
            ("d/a:strict-identity=time:b.sql", None)
        );
        for refused in [
            "koji.dump:strict-identity=tiem",
            "koji.dump:strict-identity=none,time",
            "koji.dump:strict-identity=advisory,location",
            "koji.dump:strict-identity=",
            "koji.dump:strict-identity=time:strict-identity=none",
            "koji.dump:strings:strings",
        ] {
            assert!(DumpArg::parse(refused).is_err(), "{refused}");
        }
    }

    /// **`:unrepresentable=MODE` is how this dump reads a value its column's
    /// type cannot hold**, `null` unstated, beside the other suffixes in any
    /// order and at most once.
    #[test]
    fn a_dump_states_how_it_reads_a_value_its_type_cannot_hold() {
        let mode = |arg: &str| DumpArg::parse(arg).unwrap().unrepresentable;
        assert_eq!(mode("koji.dump"), UnrepresentableMode::Null);
        assert_eq!(mode("koji.dump:unrepresentable=null"), UnrepresentableMode::Null);
        assert_eq!(mode("koji.dump:unrepresentable=text"), UnrepresentableMode::Text);
        for arg in [
            "koji.dump:unrepresentable=refuse:strings:strict-identity=none",
            "koji.dump:strict-identity=none:unrepresentable=refuse:strings",
            "koji.dump:strings:strict-identity=none:unrepresentable=refuse",
        ] {
            let parsed = parsed(arg);
            assert_eq!(
                (
                    parsed.source.as_str(),
                    parsed.schema_mode,
                    parsed.unrepresentable,
                    parsed.strict_identity
                ),
                (
                    "koji.dump",
                    SchemaMode::Strings,
                    UnrepresentableMode::Refuse,
                    Some(StrictIdentity::NONE)
                ),
                "{arg}"
            );
        }
        for refused in [
            "koji.dump:unrepresentable=typed",
            "koji.dump:unrepresentable=",
            "koji.dump:unrepresentable=null:unrepresentable=refuse",
        ] {
            assert!(DumpArg::parse(refused).is_err(), "{refused}");
        }
    }

    /// **`:postgres-invalid-values=MODE` is how this dump reads a value
    /// PostgreSQL refuses**, `default` unstated, beside the other suffixes in
    /// any order and at most once.
    #[test]
    fn a_dump_states_how_it_reads_a_value_postgresql_refuses() {
        let mode = |arg: &str| DumpArg::parse(arg).unwrap().postgres_invalid_values;
        assert_eq!(mode("koji.dump"), PostgresInvalidValues::Default);
        assert_eq!(
            mode("koji.dump:postgres-invalid-values=default"),
            PostgresInvalidValues::Default
        );
        for arg in [
            "koji.dump:postgres-invalid-values=ignore:unrepresentable=text:strings",
            "koji.dump:strings:postgres-invalid-values=ignore:unrepresentable=text",
            "koji.dump:unrepresentable=text:strings:postgres-invalid-values=ignore",
        ] {
            let parsed = parsed(arg);
            assert_eq!(
                (
                    parsed.source.as_str(),
                    parsed.schema_mode,
                    parsed.unrepresentable,
                    parsed.postgres_invalid_values
                ),
                (
                    "koji.dump",
                    SchemaMode::Strings,
                    UnrepresentableMode::Text,
                    PostgresInvalidValues::Ignore
                ),
                "{arg}"
            );
        }
        for refused in [
            "koji.dump:postgres-invalid-values=strict",
            "koji.dump:postgres-invalid-values=",
            "koji.dump:postgres-invalid-values=ignore:postgres-invalid-values=default",
        ] {
            assert!(DumpArg::parse(refused).is_err(), "{refused}");
        }
    }
}
