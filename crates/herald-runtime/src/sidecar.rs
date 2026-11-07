//! The managed `llama-server` sidecar: loopback-only, per-run API token, random
//! port, startup deadline, clean shutdown on drop.

use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::json;
use tokio::time::{Instant, sleep};

use crate::RuntimeError;
use crate::inference::{chat_completion_body, post_chat_completion, response_format};

/// Configuration for the managed `llama-server` process.
///
/// There is intentionally no bind-address field. The only legal bind is
/// loopback, and it is hardcoded so a config mistake cannot expose the model to
/// the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarConfig {
    pub binary: PathBuf,
    pub model: PathBuf,
    /// `0` means "pick a free loopback port".
    pub port: u16,
    pub api_key: String,
    pub context_size: u32,
    pub threads: u16,
}

impl SidecarConfig {
    pub fn new(binary: impl Into<PathBuf>, model: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
            model: model.into(),
            port: 0,
            api_key: random_token(),
            context_size: 4_096,
            threads: 4,
        }
    }

    pub fn validate(&self) -> Result<(), RuntimeError> {
        if self.api_key.trim().is_empty() {
            return Err(RuntimeError::InvalidConfig(
                "a per-run API token is required".into(),
            ));
        }
        if self.context_size < 512 {
            return Err(RuntimeError::InvalidConfig(
                "context size must be at least 512".into(),
            ));
        }
        Ok(())
    }

    /// Arguments passed to `llama-server`. Host is always loopback.
    #[must_use]
    pub fn arguments(&self, port: u16) -> Vec<String> {
        vec![
            "--model".into(),
            self.model.display().to_string(),
            "--host".into(),
            "127.0.0.1".into(),
            "--port".into(),
            port.to_string(),
            "--ctx-size".into(),
            self.context_size.to_string(),
            "--threads".into(),
            self.threads.to_string(),
            // `--jinja` makes the server apply the model's own chat template.
            // Without it a reasoning model never sees the template's thinking
            // switch, spends the whole token budget inside the reasoning block,
            // and returns an empty `content` with `finish_reason: "length"`.
            "--jinja".into(),
            "--api-key".into(),
            self.api_key.clone(),
        ]
    }
}

fn random_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// Ask the OS for a free loopback port. Small race window, acceptable for a
/// local child process started immediately afterwards.
pub(crate) fn free_loopback_port() -> Result<u16, RuntimeError> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    Ok(listener.local_addr()?.port())
}

/// Handle to a running, loopback-only inference server.
pub struct LlamaSidecar {
    child: Child,
    endpoint: String,
    api_key: String,
    client: reqwest::Client,
    /// Where the child's stderr is redirected for diagnostics.
    stderr_path: PathBuf,
}

impl std::fmt::Debug for LlamaSidecar {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LlamaSidecar")
            .field("endpoint", &self.endpoint)
            .field("pid", &self.child.id())
            .finish_non_exhaustive()
    }
}

impl LlamaSidecar {
    /// Spawn the server and wait until `/health` reports ready.
    pub async fn start(config: &SidecarConfig) -> Result<Self, RuntimeError> {
        config.validate()?;

        if !config.binary.exists() {
            return Err(RuntimeError::BinaryMissing(config.binary.clone()));
        }
        if !config.model.exists() {
            return Err(RuntimeError::ModelMissing(config.model.clone()));
        }

        let port = if config.port == 0 {
            free_loopback_port()?
        } else {
            config.port
        }; // A failed start used to be silent (`Stdio::null()`), which made a
        // missing chat template or a corrupt GGUF look like a timeout. The
        // child's stderr goes to a per-run file in the temp directory and only a
        // bounded tail is ever read back; the file is removed on shutdown.
        let stderr_path = std::env::temp_dir().join(format!(
            "herald-llama-{}.log",
            uuid::Uuid::new_v4().simple()
        ));
        let stderr_file = std::fs::File::create(&stderr_path)?;

        let child = Command::new(&config.binary)
            .args(config.arguments(port))
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr_file))
            .spawn()?;

        let endpoint = format!("http://127.0.0.1:{port}");
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()?;

        let mut sidecar = Self {
            child,
            endpoint,
            api_key: config.api_key.clone(),
            client,
            stderr_path,
        };

        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            if sidecar.health().await {
                return Ok(sidecar);
            }
            if Instant::now() >= deadline {
                let detail = sidecar.stderr_detail();
                sidecar.shutdown();
                return Err(RuntimeError::StartupTimeout(detail));
            }
            if let Ok(Some(status)) = sidecar.child.try_wait() {
                let detail = sidecar.stderr_detail();
                sidecar.shutdown();
                return Err(RuntimeError::InvalidConfig(format!(
                    "inference server exited early with {status}: {detail}"
                )));
            }
            sleep(Duration::from_millis(250)).await;
        }
    }

    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// The last few kilobytes the child wrote to stderr, or an empty string.
    ///
    /// Only used to explain a failed start; never part of a prompt.
    #[must_use]
    pub fn stderr_tail(&self) -> String {
        const TAIL_BYTES: u64 = 4_096;
        let Ok(mut file) = std::fs::File::open(&self.stderr_path) else {
            return String::new();
        };
        let length = file.metadata().map_or(0, |meta| meta.len());
        if length > TAIL_BYTES {
            file.seek(SeekFrom::Start(length - TAIL_BYTES)).ok();
        }
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).ok();
        String::from_utf8_lossy(&buffer).trim().to_string()
    }

    /// `stderr_tail` phrased for an error message.
    fn stderr_detail(&self) -> String {
        let tail = self.stderr_tail();
        if tail.is_empty() {
            "no output on stderr".to_string()
        } else {
            format!("stderr: {tail}")
        }
    }

    pub async fn health(&self) -> bool {
        let response = self
            .client
            .get(format!("{}/health", self.endpoint))
            .bearer_auth(&self.api_key)
            .send()
            .await;

        response.is_ok_and(|response| response.status().is_success())
    }

    /// Send a chat completion and return the assistant text.
    ///
    /// When `schema` is given the server grammar-constrains generation to that
    /// JSON schema, which is how a small local model is prevented from
    /// inventing keys or operations.
    pub async fn complete_json(
        &self,
        system: &str,
        user: &str,
        max_tokens: u16,
        schema: Option<&serde_json::Value>,
    ) -> Result<String, RuntimeError> {
        // A reasoning model spends its whole budget inside the thinking
        // block otherwise, so the answer never arrives. This is a template
        // variable, applied because the sidecar runs with `--jinja`.
        let body = chat_completion_body(
            None,
            system,
            user,
            max_tokens,
            Some(response_format(schema)),
            Some(json!({"chat_template_kwargs": {"enable_thinking": false}})),
        );

        post_chat_completion(
            &self.client,
            &format!("{}/v1/chat/completions", self.endpoint),
            Some(&self.api_key),
            &body,
        )
        .await
    }

    /// Send a plain-text chat completion and return the assistant text.
    ///
    /// Used for tier 3 drafting: plain text in, plain text out, no schema and
    /// no JSON mode anywhere on this path.
    pub async fn complete_text(
        &self,
        system: &str,
        user: &str,
        max_tokens: u16,
    ) -> Result<String, RuntimeError> {
        let body = chat_completion_body(
            None,
            system,
            user,
            max_tokens,
            None,
            Some(json!({"chat_template_kwargs": {"enable_thinking": false}})),
        );

        post_chat_completion(
            &self.client,
            &format!("{}/v1/chat/completions", self.endpoint),
            Some(&self.api_key),
            &body,
        )
        .await
    }

    /// Terminate the child, reap it, and drop its stderr file.
    pub fn shutdown(&mut self) {
        self.child.kill().ok();
        self.child.wait().ok();
        std::fs::remove_file(&self.stderr_path).ok();
    }
}

impl Drop for LlamaSidecar {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Find an executable by name on `PATH`, so a normal `llama-server` install
/// works without an absolute path in the environment.
#[must_use]
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    find_in_paths(name, std::env::split_paths(&path))
}

/// Pure half of [`find_on_path`], testable without touching the environment.
#[must_use]
pub fn find_in_paths(name: &str, paths: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    paths
        .into_iter()
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_binary_is_found_on_a_path_list() {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("llama-server");
        std::fs::write(&binary, b"#!/bin/sh\n").unwrap();
        let found = find_in_paths("llama-server", [dir.path().to_path_buf()]);
        assert_eq!(found.as_deref(), Some(binary.as_path()));
        assert!(
            find_in_paths("definitely-not-a-binary-here", [dir.path().to_path_buf()]).is_none()
        );
    }

    #[test]
    fn arguments_bind_to_loopback_only() {
        let config = SidecarConfig::new("/bin/llama-server", "/models/qwen.gguf");
        let arguments = config.arguments(1234);
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["--host", "127.0.0.1"])
        );
        assert!(!arguments.iter().any(|argument| argument == "0.0.0.0"));
    }

    #[test]
    fn arguments_apply_the_chat_template() {
        // Without `--jinja` the template's thinking switch is ignored, and a
        // reasoning model answers with empty content after the token budget.
        let config = SidecarConfig::new("/bin/llama-server", "/models/qwen.gguf");
        assert!(config.arguments(1234).iter().any(|arg| arg == "--jinja"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn early_exit_reports_the_child_stderr() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("failing-server");
        std::fs::write(
            &binary,
            "#!/usr/bin/env bash\necho 'failed to load model: bad magic' >&2\nexit 3\n",
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        let model = dir.path().join("model.gguf");
        std::fs::write(&model, b"not a gguf").unwrap();

        let config = SidecarConfig::new(&binary, &model);
        let error = LlamaSidecar::start(&config).await.unwrap_err().to_string();
        assert!(error.contains("exited early"), "{error}");
        assert!(error.contains("bad magic"), "{error}");
    }

    #[test]
    fn empty_api_key_is_rejected() {
        let mut config = SidecarConfig::new("/bin/llama-server", "/models/qwen.gguf");
        config.api_key.clear();
        assert!(config.validate().is_err());
    }

    #[test]
    fn free_port_is_usable() {
        let port = free_loopback_port().unwrap();
        assert!(port > 0);
    }

    #[test]
    fn token_is_not_reused() {
        assert_ne!(super::random_token(), super::random_token());
    }
}
