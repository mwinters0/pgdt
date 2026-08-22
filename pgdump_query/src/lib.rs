pub mod batch;
pub mod copy;
mod error;
pub mod index;
mod io;
pub mod scan;

pub use batch::{BatchOptions, read_table};
pub use copy::CopyHeader;
pub use error::Error;
pub use index::{CopyBlock, DumpIndex, build_index};
pub use io::{ByteRangeSource, LocalFileSource};
pub use scan::{CopyEnd, CopyScanner, CopyStart, Event, Row, ScanOptions, scan};

pub type Result<T> = std::result::Result<T, Error>;
