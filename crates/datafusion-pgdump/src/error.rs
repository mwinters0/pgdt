//! What opening or registering a dump refuses, the front end's words a
//! refusal carries for its remedy, and the one way anything of ours crosses
//! DataFusion's boundary.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use datafusion::common::DataFusionError;
use pgdt_model::{PostgresInvalidValues, TableName};

/// **A refusal of a dump, and the front end's words for its remedy where it
/// handed any** — the one error every path of this crate returns and the top
/// of what DataFusion carries (`docs/design/decisions.md`, "D74"). An
/// embedder matches [`Error::refusal`]; a front end that hands the dump its
/// [`Spelling`] has every refusal carry that spelling, closing its message in
/// parentheses ("D107").
///
/// Its fields are private, so a refusal is spelled only on its way out of
/// what the spelling was handed to, and at most once. Its
/// [`source`](std::error::Error::source) is the refusal's own: the library's
/// error beneath [`Refusal::Library`] and [`Refusal::MetadataLevel`], and none
/// beneath the rest.
#[derive(Debug)]
pub struct Error {
    /// Boxed so that a `Result` carrying the error stays small.
    refusal: Box<Refusal>,
    spelling: Option<String>,
}

impl Error {
    /// What was refused.
    pub fn refusal(&self) -> &Refusal {
        &self.refusal
    }

    /// The front end's words for the remedy, as its [`Spelling`] answered;
    /// `None` where it handed none, or had none for this refusal.
    pub fn spelling(&self) -> Option<&str> {
        self.spelling.as_deref()
    }
}

/// What opening or registering a dump can refuse.
///
/// **Each sentence states the condition and names its remedy in the
/// library's terms** — a mapping pass, a mode, a name given or left out —
/// never a front end's flag or command; what a front end needs to spell one
/// is in the variant's fields (`docs/design/decisions.md`, "D107"). It is
/// carried as an [`Error`], never alone.
#[derive(Debug)]
pub enum Refusal {
    /// The library's own refusal: a source it cannot read, a cache it cannot
    /// use, a table whose blocks disagree. Its sentence is the library's, and
    /// the library's error is the [`Error`]'s source, so a walk of
    /// [`std::error::Error::source`] reaches it.
    Library(pgdt_core::Error),
    /// The cache this provider reads does not cover the whole file, or is not
    /// there to read. The provider never maps, so the remedy is `pass`, run
    /// to the file's end (`docs/design/decisions.md`, "D90").
    CacheNotComplete { pass: MappingPass, why: String },
    /// A table the cache holds at the metadata level, opened typed: the
    /// library's refusal ([`pgdt_core::Error::TableAtMetadataLevel`]), the
    /// [`Error`]'s source, beside `pass`, which maps `table` again at the data
    /// level (`docs/design/decisions.md`, "D35").
    MetadataLevel { refused: Box<pgdt_core::Error>, pass: MappingPass, table: Box<TableName> },
    /// A file naming no database registered with no catalog name, where
    /// nothing else supplies one.
    CatalogNameNeeded { origin: String },
    /// A catalog name given for a file of several databases, where it could
    /// name none of them.
    CatalogNameRefused { origin: String, databases: usize, name: String },
    /// A file of several databases one of which has no name, so no catalog
    /// name can be taken from the file for it, and none may be given.
    DatabaseUnnamed { origin: String },
    /// The table asked for is not in the dump. One its selector leaves
    /// ambiguous is the library's refusal
    /// ([`pgdt_core::Error::AmbiguousTable`]).
    Table(String),
}

/// **The mapping pass that builds the cache a dump reads**, as a refusal
/// naming it carries it for a front end to spell: the dump as the open was
/// handed it, the cache path stated for it, and the mode it reads a field
/// PostgreSQL refuses under — under `Ignore`, the pass must go past such a
/// field too, where the default pass fails at the first it keys
/// (`docs/design/decisions.md`, "D103").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MappingPass {
    /// The dump, as [`crate::PgDump::open`] was handed it.
    pub location: String,
    /// The cache path the open stated, `None` where it read the default one.
    pub cache_path: Option<PathBuf>,
    /// The mode the dump was opened under.
    pub postgres_invalid_values: PostgresInvalidValues,
}

impl MappingPass {
    /// Whether the pass must go past a field PostgreSQL refuses, where the
    /// default pass fails at the first it keys.
    pub fn goes_past_refused_values(&self) -> bool {
        self.postgres_invalid_values == PostgresInvalidValues::Ignore
    }
}

/// How the pass reads a field PostgreSQL refuses, said where it is not the
/// default pass's way.
const GOING_PAST: &str = "going past the values PostgreSQL refuses, as this dump reads them";

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::Library(err) => write!(f, "{err}"),
            Refusal::CacheNotComplete { pass, why } => {
                write!(
                    f,
                    "{why}; the provider reads only a complete cache, which a mapping pass to \
                     the file's end builds"
                )?;
                if pass.goes_past_refused_values() {
                    write!(f, ", {GOING_PAST}")?;
                }
                Ok(())
            }
            Refusal::MetadataLevel { refused, pass, .. } => {
                write!(f, "{refused}")?;
                if pass.goes_past_refused_values() {
                    write!(f, "; that mapping pass maps it {GOING_PAST}")?;
                }
                Ok(())
            }
            Refusal::CatalogNameNeeded { origin } => {
                write!(f, "{origin} names no database, so its catalog needs a name — give one")
            }
            Refusal::CatalogNameRefused { origin, databases, name } => write!(
                f,
                "{origin} holds {databases} databases, each registered under its own name; \
                 `{name}` would name none of them — leave the name out"
            ),
            Refusal::DatabaseUnnamed { origin } => write!(
                f,
                "{origin} holds several databases and one of them has no name, so no catalog \
                 name can be taken from the file for it"
            ),
            Refusal::Table(why) => f.write_str(why),
        }
    }
}

/// The refusal, closed by the front end's spelling of its remedy in
/// parentheses where it has one.
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.refusal)?;
        match &self.spelling {
            Some(spelling) => write!(f, " ({spelling})"),
            None => Ok(()),
        }
    }
}

/// The refusal's own source: the [`Refusal`] is not an error of its own, so a
/// walk reaches the library's error in one step, spelled or not.
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self.refusal.as_ref() {
            Refusal::Library(err) => Some(err),
            Refusal::MetadataLevel { refused, .. } => Some(refused.as_ref()),
            Refusal::CacheNotComplete { .. }
            | Refusal::CatalogNameNeeded { .. }
            | Refusal::CatalogNameRefused { .. }
            | Refusal::DatabaseUnnamed { .. }
            | Refusal::Table(_) => None,
        }
    }
}

/// A refusal no spelling has been asked of.
impl From<Refusal> for Error {
    fn from(refusal: Refusal) -> Self {
        Error { refusal: Box::new(refusal), spelling: None }
    }
}

/// The library's refusal, unspelled.
impl From<pgdt_core::Error> for Error {
    fn from(err: pgdt_core::Error) -> Self {
        Refusal::Library(err).into()
    }
}

/// **A front end's words for the remedy a refusal names in the library's
/// terms** — its flag, option or command — or `None` where it has none.
/// Handed to the provider where a dump is opened
/// ([`PgDumpOptions::spelling`](crate::PgDumpOptions::spelling)) or a
/// factory registered ([`crate::register_table_factory`]), so that every
/// refusal of what they build carries it in its message before it crosses
/// DataFusion's boundary, wherever a front end prints it: a refusal a
/// statement meets is printed by DataFusion's own machinery, out of the
/// front end's reach (`docs/design/decisions.md`, "D107").
///
/// It is asked once of each refusal, the library's beneath
/// [`Refusal::Library`].
pub trait Spelling: Send + Sync {
    fn spell(&self, refused: &Refusal) -> Option<String>;
}

impl<F> Spelling for F
where
    F: Fn(&Refusal) -> Option<String> + Send + Sync,
{
    fn spell(&self, refused: &Refusal) -> Option<String> {
        self(refused)
    }
}

impl fmt::Debug for dyn Spelling {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Spelling")
    }
}

/// The spelling a dump or a factory was handed, applied to each refusal on
/// its way out — the only place an [`Error`] gains a spelling.
#[derive(Debug, Clone, Default)]
pub(crate) struct Speller(pub(crate) Option<Arc<dyn Spelling>>);

impl Speller {
    /// `refused`, closed by the front end's spelling where it has one; one
    /// already spelled is left as it is.
    pub(crate) fn error(&self, refused: impl Into<Error>) -> Error {
        let mut refused = refused.into();
        if refused.spelling.is_none() {
            refused.spelling =
                self.0.as_deref().and_then(|spelling| spelling.spell(&refused.refusal));
        }
        refused
    }

    /// The library's refusal across DataFusion's boundary, spelled.
    pub(crate) fn external(&self, refused: pgdt_core::Error) -> DataFusionError {
        self.error(refused).into()
    }
}

/// **Every refusal of a dump or of the library crosses DataFusion's boundary
/// as [`DataFusionError::External`] holding an [`Error`]**, from a scan, a
/// pushed filter, a table built or registered and a `CREATE EXTERNAL TABLE`
/// alike, so an embedder downcasts the top to [`Error`] on every path and
/// reaches `pgdt_core::Error` below it by [`std::error::Error::source`]
/// (`docs/design/decisions.md`, "D74"). A statement or a setting refused for
/// its own shape — columns declared, an unknown `pgdump.` option — is
/// DataFusion's plan error.
impl From<Error> for DataFusionError {
    fn from(err: Error) -> Self {
        DataFusionError::External(Box::new(err))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A refusal is spelled once**: one already spelled keeps its spelling
    /// whatever speller it passes next, and one a spelling has no words for
    /// says the refusal alone.
    #[test]
    fn a_refusal_is_spelled_once() {
        let spelling = |words: &'static str| {
            Speller(Some(Arc::new(move |_: &Refusal| Some(words.to_string()))))
        };
        let refused = || Refusal::Table("no table `x`".to_string());
        let spelled = spelling("first").error(refused());
        let respelled = spelling("second").error(spelled);
        assert_eq!(respelled.spelling(), Some("first"));
        assert_eq!(respelled.to_string(), "no table `x` (first)");
        let silent = Speller(Some(Arc::new(|_: &Refusal| None)));
        assert_eq!(silent.error(refused()).to_string(), "no table `x`");
        assert_eq!(Speller::default().error(refused()).spelling(), None);
    }
}
