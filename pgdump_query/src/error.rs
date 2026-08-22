use thiserror::Error as ThisError;

#[derive(Debug, ThisError)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("background task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("COPY block at offset {header_offset} has no terminating `\\.` line")]
    UnterminatedCopyBlock { header_offset: u64 },
    #[error("line at offset {offset} exceeds the {limit}-byte line limit")]
    LineTooLong { offset: u64, limit: usize },
    #[error("value is not valid UTF-8 (valid up to byte {valid_up_to})")]
    InvalidUtf8 { valid_up_to: usize },
}
