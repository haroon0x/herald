//! Local inference runtime.
//!
//! Herald manages its own pinned `llama-server` child process. It binds to
//! loopback only, uses a random per-run token, and never requires the user to
//! install Ollama or a Python stack.
//!
//! The [`Inference`] port keeps the model tier testable and usable without a
//! bundled binary. [`LlamaSidecar`] remains the default; [`OpenAiClient`] is an
//! explicit opt-in for an already-installed local OpenAI-compatible endpoint.

pub mod download;
pub mod inference;
pub mod manifest;
pub mod opencode;
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
    #[error("download failed: HTTP {status} for {url}")]
    DownloadFailed { status: u16, url: String },
    #[error("the server does not support resumable downloads (range unsupported): {url}")]
    RangeUnsupported { url: String },
    #[error("the inference server did not become ready in time: {0}")]
    StartupTimeout(String),
    #[error("the inference server returned an invalid response: {0}")]
    BadResponse(String),
    #[error("the model process failed: {0}")]
    ModelProcess(String),
    #[error("inference is not configured: {0}")]
    NotConfigured(String),
}

pub use download::{DownloadStats, download_model, download_url};
pub use inference::{Inference, OpenAiClient};
pub use manifest::ModelManifest;
pub use opencode::OpenCodeInference;
pub use sidecar::{
    ChatMessage, LlamaSidecar, SidecarConfig, binary_from_env, free_loopback_port, model_path,
};
