//! The shell's spelling of each refusal's remedy, in the grammar of whatever
//! registered the dump: a `--dump`'s suffixes, or a `STORED AS PGDUMP`
//! statement's options; and `pgdt`'s own commands where the remedy is a
//! parse (`docs/design/decisions.md`, "D107").

use datafusion_pgdump::{Error, MappingPass, Spelling};
use pgdt_model::TableName;

/// Which of the shell's two grammars a dump was registered in, and so which
/// one spells its refusals: the provider is handed one at construction, so a
/// refusal carries its spelling in its message wherever DataFusion's CLI
/// prints it, the REPL and `-f` included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Grammar {
    /// `--dump [NAME=]SOURCE[:strings]…`.
    Dump,
    /// `CREATE EXTERNAL TABLE … STORED AS PGDUMP … OPTIONS ('pgdump.…' …)`.
    Statement,
}

impl Spelling for Grammar {
    fn spell(&self, refused: &Error) -> Option<String> {
        spelling(*self, refused)
    }
}

/// **The shell's words for the remedy `refused` names**, in the library's
/// order, or `None` where it has none.
///
/// **Exhaustive over the provider's variants and the library's**, as `pgdt`'s
/// own `refusal::spelling` is: whether a remedy has a spelling here is a
/// property of each variant, so one added to either does not compile until
/// it has been spelled or said to need none.
pub(crate) fn spelling(grammar: Grammar, refused: &Error) -> Option<String> {
    let option = |key: &str, value: &str| format!("`'pgdump.{key}' '{value}'`");
    let spelled = match (refused, grammar) {
        (Error::Library(refused), _) => return library(grammar, refused),
        (Error::CacheNotComplete { pass, .. }, _) => format!("`{}`", parse(pass, None)),
        (Error::MetadataLevel { pass, table, .. }, Grammar::Dump) => {
            format!("`{}`; `:strings`", parse(pass, Some(table)))
        }
        (Error::MetadataLevel { pass, table, .. }, Grammar::Statement) => {
            format!("`{}`; {}", parse(pass, Some(table)), option("schema_mode", "strings"))
        }
        (Error::CatalogNameNeeded { .. }, Grammar::Dump) => "`--dump NAME=SOURCE`".to_string(),
        (Error::CatalogNameRefused { .. }, Grammar::Dump) => {
            "`--dump SOURCE`, without `NAME=`".to_string()
        }
        // A statement registers one table, never a catalog.
        (Error::CatalogNameNeeded { .. } | Error::CatalogNameRefused { .. }, Grammar::Statement)
        // No name can be given, and none taken from the file.
        | (Error::DatabaseUnnamed { .. }, _)
        // The table asked for is not there: the user's own act.
        | (Error::Table(_), _)
        | (Error::Spelled { .. }, _) => return None,
    };
    Some(spelled)
}

/// The library's refusal, as [`spelling`] spells it.
fn library(grammar: Grammar, refused: &pgdt_core::Error) -> Option<String> {
    use pgdt_core::Error as Lib;
    let (dump, statement) = match refused {
        Lib::SourceChangedWhileRead { .. } | Lib::SourceUncheckable { .. } => {
            ("`:strict-identity=none`", "`'pgdump.strict_identity' 'none'`".to_string())
        }
        Lib::StrictIdentityUnmet { term, .. } => {
            return Some(match grammar {
                Grammar::Dump => format!("`pgdt parse`; `:strict-identity=` without `{term}`"),
                Grammar::Statement => {
                    format!("`pgdt parse`; `'pgdump.strict_identity'` without `{term}`")
                }
            });
        }
        // The shell names no cache path, so of the three ways out it spells
        // the scan replacing it; a file that is not a pgdt cache no scan
        // replaces (`docs/design/decisions.md`, "D20").
        Lib::CacheUnusable { unusable, .. } if unusable.overwritable() => {
            return Some("`pgdt parse --overwrite-unusable-cache`".to_string());
        }
        Lib::CachedBlockChanged { path: Some(_), .. } => return Some("`pgdt parse`".to_string()),
        Lib::MetadataNotScanned { .. } => {
            ("`pgdt parse`; `:strings`", "`pgdt parse`; `'pgdump.schema_mode' 'strings'`".into())
        }
        Lib::FieldDecode { .. } => ("`:strings`", "`'pgdump.schema_mode' 'strings'`".to_string()),
        Lib::TableAtMetadataLevel { .. } => (
            "`pgdt parse --statistics-level` naming it `data`; `:strings`",
            "`pgdt parse --statistics-level` naming it `data`; `'pgdump.schema_mode' 'strings'`"
                .to_string(),
        ),
        Lib::Unrepresentable { .. } => (
            "`:unrepresentable=null`; `:unrepresentable=text`",
            "`'pgdump.unrepresentable' 'null'`; `'pgdump.unrepresentable' 'text'`".to_string(),
        ),
        Lib::TestNeedsDeclaredType { .. } => {
            ("`--dump` without `:strings`", "`'pgdump.schema_mode' 'typed'`".to_string())
        }
        // Only a statement names a table by a selector that can match two.
        Lib::AmbiguousTable { .. } => match grammar {
            Grammar::Dump => return None,
            Grammar::Statement => return Some("`'pgdump.schema'`; `'pgdump.database'`".into()),
        },
        // The rest of an enum's labels are in its declaration, which `info
        // --detail` prints.
        Lib::PredicateValueDecode { unlisted, .. } if *unlisted > 0 => {
            return Some("`pgdt info --detail`".to_string());
        }
        // No remedy, or none the shell has words for: the condition alone, a
        // remedy that is the user's own act, one the sentence states beside
        // the request it refuses, or one the provider never meets — it
        // always names a cache, never resumes and never maps.
        Lib::CacheUnusable { .. }
        | Lib::CachedBlockChanged { path: None, .. }
        | Lib::CacheDisabled { .. }
        | Lib::Io(_)
        | Lib::Join(_)
        | Lib::UnterminatedCopyBlock { .. }
        | Lib::UnterminatedLargeObjectRegion { .. }
        | Lib::LineTooLong { .. }
        | Lib::ScanCancelled { .. }
        | Lib::InvalidUtf8 { .. }
        | Lib::ColumnCountMismatch { .. }
        | Lib::Arrow(_)
        | Lib::SourceNotReadable { .. }
        | Lib::Remote { .. }
        | Lib::Xz(_)
        | Lib::CacheEncode(_)
        | Lib::UnknownPredicateColumn { .. }
        | Lib::UnorderedPredicateColumn { .. }
        | Lib::UncomparablePredicateColumn { .. }
        | Lib::PredicateValueDecode { .. }
        | Lib::UnknownProjectionColumn { .. }
        | Lib::DuplicateProjectionColumn { .. }
        | Lib::ResumeQueryMismatch
        | Lib::TableColumnsDisagree { .. }
        | Lib::MapIncomplete { .. }
        | Lib::FieldRefused { .. }
        | Lib::RowEndingRefused { .. }
        | Lib::NullRefused { .. }
        | Lib::FieldRefusedRecorded { .. }
        | Lib::FieldRender { .. } => return None,
    };
    Some(match grammar {
        Grammar::Dump => dump.to_string(),
        Grammar::Statement => statement,
    })
}

/// The `pgdt parse` that runs `pass`, putting `data_level` at the data level
/// and keeping every other table's level, a parse never recording less than
/// a cache holds.
fn parse(pass: &MappingPass, data_level: Option<&TableName>) -> String {
    let mut parse = format!("pgdt parse --source {}", pass.location);
    if let Some(path) = &pass.cache_path {
        parse.push_str(&format!(" --dtcache {}", path.display()));
    }
    if pass.goes_past_refused_values() {
        parse.push_str(" --postgres-invalid-values ignore");
    }
    if let Some(table) = data_level {
        parse.push_str(&format!(" --statistics-level metadata,{}=data", table.qualified_name()));
    }
    parse
}

#[cfg(test)]
mod tests;
