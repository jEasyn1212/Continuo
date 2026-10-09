pub mod adapters;
pub mod api;
pub mod capability;
pub mod crypto;
pub mod identity;
pub mod mcp;
pub mod model;
pub mod store;
pub mod sync;
pub mod task;

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize)]
pub struct Error {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl Error {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: None,
        }
    }
    pub fn details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::new("io_error", e.to_string())
    }
}
impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Self::new("storage_error", e.to_string())
    }
}
impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::new("invalid_json", e.to_string())
    }
}
pub type Result<T> = std::result::Result<T, Error>;

/// All interfaces use the same default vault; tests override it explicitly.
pub fn default_data_dir() -> Result<std::path::PathBuf> {
    if let Some(path) = std::env::var_os("CONTINUO_DATA_DIR") {
        return Ok(path.into());
    }
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|home| std::path::PathBuf::from(home).join(".continuo"))
        .ok_or_else(|| {
            Error::new(
                "invalid_path",
                "Set CONTINUO_DATA_DIR or an explicit data directory",
            )
        })
}
