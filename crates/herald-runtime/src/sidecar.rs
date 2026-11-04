//! The managed `llama-server` sidecar: loopback-only, per-run API token, random
//! port, startup deadline, clean shutdown on drop.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};
use tokio::time::{Instant, sleep};

use crate::RuntimeError;

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
pub fn free_loopback_port() -> Result<u16, RuntimeError> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    Ok(listener.local_addr()?.port())
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    #[must_use]
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: content.into(),
        }
    }

    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: content.into(),
        }
    }
}

/// Handle to a running, loopback-only inference server.
pub struct LlamaSidecar {
    child: Child,
    endpoint: String,
    api_key: String,
    client: reqwest::Client,
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
        };

        let child = Command::new(&config.binary)
            .args(config.arguments(port))
            // Keep the child quiet; a real logging sink lands in Phase 1.
            .stdout(Stdio::null())
            .stderr(Stdio::null())
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
        };

        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            if sidecar.health().await {
                return Ok(sidecar);
            }
            if Instant::now() >= deadline {
                sidecar.shutdown();
                return Err(RuntimeError::StartupTimeout);
            }
            if let Ok(Some(status)) = sidecar.child.try_wait() {
                return Err(RuntimeError::InvalidConfig(format!(
                    "inference server exited early with {status}"
                )));
            }
            sleep(Duration::from_millis(250)).await;
        }
    }

    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
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
        let response_format = match schema {
            None => json!({"type": "json_object"}),
            Some(schema) => json!({
                "type": "json_schema",
                "json_schema": {
                    "name": "ActionPlan",
                    "strict": true,
                    "schema": schema,
                }
            }),
        };

        let body = json!({
            "messages": [
                ChatMessage::system(system),
                ChatMessage::user(user),
            ],
            "max_tokens": max_tokens,
            "temperature": 0.0,
            "response_format": response_format,
            "stream": false,
        });

        self.complete(body).await
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
        let body = json!({
            "messages": [
                ChatMessage::system(system),
                ChatMessage::user(user),
            ],
            "max_tokens": max_tokens,
            "temperature": 0.0,
            "stream": false,
        });

        self.complete(body).await
    }

    /// Post one chat-completion body and extract the assistant text.
    async fn complete(&self, body: Value) -> Result<String, RuntimeError> {
        let response = self
            .client
            .post(format!("{}/v1/chat/completions", self.endpoint))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        let raw = response.text().await?;
        if !status.is_success() {
            return Err(RuntimeError::BadResponse(format!("{status}: {raw}")));
        }

        let parsed: Value = serde_json::from_str(&raw)
            .map_err(|error| RuntimeError::BadResponse(format!("{error}: {raw}")))?;

        parsed["choices"][0]["message"]["content"]
            .as_str()
            .map(ToString::to_string)
            .ok_or_else(|| RuntimeError::BadResponse("missing assistant content".into()))
    }

    /// Terminate the child and reap it.
    pub fn shutdown(&mut self) {
        self.child.kill().ok();
        self.child.wait().ok();
    }
}

impl Drop for LlamaSidecar {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Resolve the llama-server binary from `HERALD_LLAMA_SERVER` or a path.
#[must_use]
pub fn binary_from_env() -> Option<PathBuf> {
    std::env::var_os("HERALD_LLAMA_SERVER").map(PathBuf::from)
}

/// Where downloaded models live for a given data directory.
#[must_use]
pub fn model_path(data_dir: &Path, filename: &str) -> PathBuf {
    data_dir.join("models").join(filename)
}

#[cfg(test)]
mod tests {
    use super::*;

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
