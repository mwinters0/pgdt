mod error;
mod io;

pub use error::Error;
pub use io::{ByteRangeSource, LocalFileSource};

pub type Result<T> = std::result::Result<T, Error>;
