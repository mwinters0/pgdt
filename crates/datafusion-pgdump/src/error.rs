//! What opening or registering a dump refuses, and the library's refusal
//! carried across DataFusion's boundary.

use datafusion::common::DataFusionError;

/// What opening or registering a dump can refuse.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The library's own refusal: a source it cannot read, a cache it cannot
    /// use, a table whose blocks disagree.
    #[error(transparent)]
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
    #[error(transparent)]
    DataFusion(#[from] datafusion::error::DataFusionError),
}

/// A library error carried across DataFusion's boundary whole, so an embedder
/// can downcast it back.
pub(crate) fn external(err: pgdt_core::Error) -> DataFusionError {
    DataFusionError::External(Box::new(err))
}
