//! What opening or registering a dump refuses, and the one way anything of
//! ours crosses DataFusion's boundary.

use datafusion::common::DataFusionError;

/// What opening or registering a dump can refuse.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The library's own refusal: a source it cannot read, a cache it cannot
    /// use, a table whose blocks disagree. Its sentence is the library's, and
    /// the library's error is its source, so a walk of
    /// [`std::error::Error::source`] reaches it.
    #[error("{0}")]
    Library(#[from] pgdt_core::Error),
    /// The cache this provider reads does not cover the whole file, or is not
    /// there to read. The provider never maps, so the remedy is the parse
    /// named.
    #[error("{why}; the provider reads only a complete cache — run `{parse}` to build it")]
    CacheNotComplete { parse: String, why: String },
    /// A table the cache holds at the metadata level, opened typed: the cache
    /// records no census of its rows to settle its schema from, and the
    /// provider never maps, so the remedy is the parse named, which puts that
    /// table at the data level and leaves every other table as it is
    /// (`docs/design/decisions.md`, "D35").
    #[error(
        "{table} is mapped at the metadata level, which records nothing drawn from its rows, and a typed query needs its census — run `{parse}` to record it"
    )]
    MetadataLevel { table: String, parse: String },
    /// A catalog name is needed and nothing supplies one, or one was supplied
    /// for a file of several databases, where it could name none of them.
    #[error("{0}")]
    CatalogName(String),
    /// The table asked for is not in the dump, or its name alone matches more
    /// than one.
    #[error("{0}")]
    Table(String),
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

/// The library's refusal across DataFusion's boundary, as
/// [`Error::Library`].
pub(crate) fn external(err: pgdt_core::Error) -> DataFusionError {
    Error::Library(err).into()
}
