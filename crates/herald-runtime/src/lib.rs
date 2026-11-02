//! Local inference runtime.
//!
//! Herald manages its own pinned `llama-server` child process. It binds to
//! loopback only, uses a random per-run token, and never requires the user to
//! install Ollama or a Python stack.

pub mod manifest;
pub mod sidecar;

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("invalid runtime configuration: {0}")]
    InvalidConfig(String),
    #[error("model file not found: {0}")]
    ModelMissing(PathBuf),
    #[error("inference binary not found: {0}")]
    BinaryMissing(PathBuf),
    #[error("checksum mismatch for {path}: expected {expected}, got {actual}")]
    ChecksumMismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },
    #[error("the inference server did not become ready in time")]
    StartupTimeout,
    #[error("the inference server returned an invalid response: {0}")]
    BadResponse(String),
}

pub use manifest::ModelManifest;
pub use sidecar::{ChatMessage, LlamaSidecar, SidecarConfig, free_loopback_port};
