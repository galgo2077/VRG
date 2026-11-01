use std::io;

use thiserror::Error;

pub type VrgResult<T> = Result<T, VrgError>;

#[derive(Debug, Error)]
pub enum VrgError {
    #[error("I/O operation failed: {0}")]
    Io(#[from] io::Error),
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("access denied: {0}")]
    AccessDenied(String),
    #[error("path is outside its authorized root: {0}")]
    PathEscape(String),
    #[error("symbolic links are not allowed for this operation: {0}")]
    SymlinkRejected(String),
    #[error("Markdown could not be parsed: {0}")]
    Markdown(String),
    #[error("derived storage failed: {0}")]
    Storage(String),
    #[error("text retrieval failed: {0}")]
    Search(String),
    #[error("semantic retrieval is unavailable: {0}")]
    SemanticUnavailable(String),
    #[error("index generation conflict: {0}")]
    Conflict(String),
    #[error("journal recovery failed: {0}")]
    Recovery(String),
    #[error("note writes are disabled: {0}")]
    WritesDisabled(String),
    #[error("MCP protocol operation failed: {0}")]
    Mcp(String),
}
