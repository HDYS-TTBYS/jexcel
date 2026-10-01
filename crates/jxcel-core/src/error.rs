use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error in {path}: {source}")]
    Json {
        path: String,
        source: serde_json::Error,
    },
    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("invalid file: {0}")]
    Invalid(String),
    #[error("unsupported format version {0}")]
    UnsupportedVersion(u32),
    #[error("not found: {0}")]
    NotFound(String),
}

pub type Result<T> = std::result::Result<T, Error>;
