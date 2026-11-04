//! Inference port and adapters.
//!
//! [`Inference`] is the only way the rest of Herald talks to a model. The
//! managed loopback `llama-server` sidecar remains the default; [`OpenAiClient`]
//! is an explicit opt-in for an already-installed local OpenAI-compatible server
//! (Ollama, LM Studio, vLLM). There is deliberately no public fallback endpoint:
//! the client only ever talks to the URL the user configured, and no telemetry
//! or message content is written to logs.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::{LlamaSidecar, RuntimeError};

/// Total budget for one completion request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
/// Budget for establishing the TCP/TLS connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How much of an error response is echoed back in an error message.
const MAX_ERROR_BODY_CHARS: usize = 500;

/// A model that can answer JSON-constrained and plain-text completions.
///
/// Implementations must not execute anything; they return text that the caller
/// validates against its own schema.
#[async_trait]
pub trait Inference: Send + Sync + std::fmt::Debug {
    async fn complete_json(
        &self,
        system: &str,
        user: &str,
        max_tokens: u16,
        schema: Option<&serde_json::Value>,
    ) -> Result<String, RuntimeError>;

    async fn complete_text(
        &self,
        system: &str,
        user: &str,
        max_tokens: u16,
    ) -> Result<String, RuntimeError>;
}

#[async_trait]
impl Inference for LlamaSidecar {
    async fn complete_json(
        &self,
        system: &str,
        user: &str,
        max_tokens: u16,
        schema: Option<&serde_json::Value>,
    ) -> Result<String, RuntimeError> {
        LlamaSidecar::complete_json(self, system, user, max_tokens, schema).await
    }

    async fn complete_text(
        &self,
        system: &str,
        user: &str,
        max_tokens: u16,
    ) -> Result<String, RuntimeError> {
        LlamaSidecar::complete_text(self, system, user, max_tokens).await
    }
}

/// A client for an OpenAI-compatible chat-completions endpoint.
///
/// The base URL is normalised and validated once, at construction: only
/// `http`/`https` URLs with a host are accepted, so a config typo cannot turn a
/// missing endpoint into a request somewhere unintended.
pub struct OpenAiClient {
    client: reqwest::Client,
    base_url: String,
    model: String,
    api_key: Option<String>,
}

impl std::fmt::Debug for OpenAiClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OpenAiClient")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .finish_non_exhaustive()
    }
}

impl OpenAiClient {
    /// Build a client with the default timeout policy.
    pub fn new(
        base_url: impl Into<String>,
        model: impl Into<String>,
        api_key: Option<String>,
    ) -> Result<Self, RuntimeError> {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .build()?;

        Self::with_client(client, base_url, model, api_key)
    }

    /// Build a client around an existing [`reqwest::Client`].
    ///
    /// Useful for tests and for callers that already share a tuned client. The
    /// caller owns that client's timeout policy.
    pub fn with_client(
        client: reqwest::Client,
        base_url: impl Into<String>,
        model: impl Into<String>,
        api_key: Option<String>,
    ) -> Result<Self, RuntimeError> {
        let base_url = normalize_base_url(&base_url.into())?;

        let model = model.into();
        let model = model.trim();
        if model.is_empty() {
            return Err(RuntimeError::NotConfigured(
                "a model name is required".into(),
            ));
        }

        let api_key = api_key
            .map(|key| key.trim().to_string())
            .filter(|key| !key.is_empty());

        Ok(Self {
            client,
            base_url,
            model: model.to_string(),
            api_key,
        })
    }

    /// Build the common request shape. Callers add `response_format` only when
    /// they actually want JSON.
    fn chat_body(&self, system: &str, user: &str, max_tokens: u16) -> Value {
        json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
            "max_tokens": max_tokens,
            "temperature": 0.0,
            "stream": false,
        })
    }

    /// Post one chat-completion body and extract the assistant text.
    async fn complete(&self, body: &Value) -> Result<String, RuntimeError> {
        let url = format!("{}/chat/completions", self.base_url);
        let mut request = self.client.post(url).json(body);
        if let Some(api_key) = &self.api_key {
            request = request.bearer_auth(api_key);
        }

        let response = request.send().await?;
        let status = response.status();
        let raw = response.text().await?;

        if !status.is_success() {
            return Err(RuntimeError::BadResponse(format!(
                "HTTP {status}: {}",
                body_snippet(&raw)
            )));
        }

        let parsed: Value = serde_json::from_str(&raw).map_err(|error| {
            RuntimeError::BadResponse(format!(
                "invalid completion JSON ({error}): {}",
                body_snippet(&raw)
            ))
        })?;

        parsed
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("content"))
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .ok_or_else(|| {
                RuntimeError::BadResponse("completion response is missing assistant content".into())
            })
    }
}

#[async_trait]
impl Inference for OpenAiClient {
    async fn complete_json(
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

        let mut body = self.chat_body(system, user, max_tokens);
        if let Some(object) = body.as_object_mut() {
            object.insert("response_format".into(), response_format);
        }

        self.complete(&body).await
    }

    async fn complete_text(
        &self,
        system: &str,
        user: &str,
        max_tokens: u16,
    ) -> Result<String, RuntimeError> {
        let body = self.chat_body(system, user, max_tokens);
        self.complete(&body).await
    }
}

/// Validate and normalise a configured endpoint.
///
/// Accepts `http`/`https` URLs with a host, strips trailing slashes so the final
/// request path never contains an accidental `//`, and rejects query strings and
/// fragments because they cannot be combined with `/chat/completions` safely.
fn normalize_base_url(base_url: &str) -> Result<String, RuntimeError> {
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        return Err(RuntimeError::NotConfigured(
            "an inference endpoint base URL is required".into(),
        ));
    }

    let parsed = reqwest::Url::parse(trimmed).map_err(|_| {
        RuntimeError::InvalidConfig("the inference endpoint is not a valid URL".into())
    })?;

    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(RuntimeError::InvalidConfig(
            "the inference endpoint must use http or https".into(),
        ));
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err(RuntimeError::InvalidConfig(
            "the inference endpoint must include a host".into(),
        ));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(RuntimeError::InvalidConfig(
            "the inference endpoint must not include a query or fragment".into(),
        ));
    }

    Ok(parsed.as_str().trim_end_matches('/').to_string())
}

/// Render a short, single-line, bounded excerpt of an endpoint response.
///
/// Error strings are user-visible and may end up in a log, so the body is capped
/// and newlines are folded away rather than echoed verbatim.
fn body_snippet(body: &str) -> String {
    let mut snippet: String = body.chars().take(MAX_ERROR_BODY_CHARS).collect();
    if body.chars().count() > MAX_ERROR_BODY_CHARS {
        snippet.push_str("...");
    }
    snippet.replace(['\r', '\n'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{SocketAddr, TcpListener};
    use std::sync::mpsc::{Receiver, channel};

    const MODEL: &str = "qwen2.5:1.5b-instruct";

    #[derive(Debug)]
    struct CapturedRequest {
        request_line: String,
        headers: Vec<(String, String)>,
        body: String,
    }

    impl CapturedRequest {
        fn path(&self) -> &str {
            self.request_line
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
        }

        fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.as_str())
        }

        fn json(&self) -> Value {
            serde_json::from_str(&self.body).expect("request body should be JSON")
        }
    }

    /// Serve exactly one HTTP/1.1 request, capture it, then reply with `response`.
    fn spawn_server(response: String) -> (SocketAddr, Receiver<CapturedRequest>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = channel();

        std::thread::spawn(move || {
            let Ok((stream, _peer)) = listener.accept() else {
                return;
            };
            let mut writer = stream.try_clone().unwrap();
            let mut reader = BufReader::new(stream);

            let mut request_line = String::new();
            if reader.read_line(&mut request_line).is_err() {
                return;
            }

            let mut headers = Vec::new();
            let mut content_length = 0_usize;
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                if line == "\r\n" || line == "\n" {
                    break;
                }
                if let Some((key, value)) = line.split_once(':') {
                    let value = value.trim().to_string();
                    if key.trim().eq_ignore_ascii_case("content-length") {
                        content_length = value.parse().unwrap_or(0);
                    }
                    headers.push((key.trim().to_string(), value));
                }
            }

            let mut body = vec![0_u8; content_length];
            match reader.read_exact(&mut body) {
                Ok(()) | Err(_) => {}
            }

            sender
                .send(CapturedRequest {
                    request_line: request_line.trim_end().to_string(),
                    headers,
                    body: String::from_utf8_lossy(&body).into_owned(),
                })
                .ok();

            writer.write_all(response.as_bytes()).ok();
            writer.flush().ok();
        });

        (address, receiver)
    }

    fn canned(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
    }

    fn client(address: SocketAddr) -> OpenAiClient {
        OpenAiClient::new(format!("http://{address}/v1"), MODEL, None).unwrap()
    }

    #[tokio::test]
    async fn complete_json_sends_schema_and_extracts_content() {
        let (address, requests) = spawn_server(canned(
            r#"{"choices":[{"message":{"content":"{\"operation\":\"get_thread\"}"}}]}"#,
        ));
        let schema = json!({
            "type": "object",
            "properties": {"operation": {"type": "string"}},
            "required": ["operation"],
            "additionalProperties": false
        });

        let content = client(address)
            .complete_json("classify this", "open the lease email", 64, Some(&schema))
            .await
            .unwrap();
        assert_eq!(content, r#"{"operation":"get_thread"}"#);

        let request = requests.recv().unwrap();
        assert_eq!(request.request_line, "POST /v1/chat/completions HTTP/1.1");
        assert_eq!(request.path(), "/v1/chat/completions");
        assert!(request.header("authorization").is_none());

        let body = request.json();
        assert_eq!(body["model"], MODEL);
        assert_eq!(body["max_tokens"], 64);
        assert_eq!(body["temperature"], 0.0);
        assert_eq!(body["stream"], Value::Bool(false));
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][0]["content"], "classify this");
        assert_eq!(body["messages"][1]["role"], "user");
        assert_eq!(body["messages"][1]["content"], "open the lease email");
        assert_eq!(
            body["response_format"],
            json!({
                "type": "json_schema",
                "json_schema": {
                    "name": "ActionPlan",
                    "strict": true,
                    "schema": schema,
                }
            })
        );
    }

    #[tokio::test]
    async fn complete_json_without_schema_requests_json_object() {
        let (address, requests) =
            spawn_server(canned(r#"{"choices":[{"message":{"content":"{}"}}]}"#));

        let content = client(address)
            .complete_json("system", "user", 8, None)
            .await
            .unwrap();
        assert_eq!(content, "{}");

        let body = requests.recv().unwrap().json();
        assert_eq!(body["response_format"], json!({"type": "json_object"}));
    }

    #[tokio::test]
    async fn base_url_with_trailing_slash_keeps_one_slash() {
        let (address, requests) =
            spawn_server(canned(r#"{"choices":[{"message":{"content":"ok"}}]}"#));
        let client = OpenAiClient::new(format!("http://{address}/v1/"), MODEL, None).unwrap();

        client.complete_text("system", "user", 8).await.unwrap();

        let request = requests.recv().unwrap();
        assert_eq!(request.path(), "/v1/chat/completions");
        assert!(!request.request_line.contains("//"));
    }

    #[tokio::test]
    async fn complete_text_sends_no_response_format() {
        let (address, requests) = spawn_server(canned(
            r#"{"choices":[{"message":{"content":"Thanks, I'll take a look."}}]}"#,
        ));

        let text = client(address)
            .complete_text("draft a reply", "reply politely", 32)
            .await
            .unwrap();
        assert_eq!(text, "Thanks, I'll take a look.");

        let body = requests.recv().unwrap().json();
        assert!(body.get("response_format").is_none());
        assert_eq!(body["messages"][1]["content"], "reply politely");
        assert_eq!(body["stream"], Value::Bool(false));
    }

    #[tokio::test]
    async fn http_error_reports_status_and_truncated_single_line_body() {
        let body = format!("first-line\n{}TAIL-MARKER", "x".repeat(800));
        let response = format!(
            "HTTP/1.1 503 Service Unavailable\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let (address, _requests) = spawn_server(response);

        let error = client(address)
            .complete_json("system", "user", 8, None)
            .await
            .unwrap_err();
        let message = error.to_string();

        assert!(message.contains("503"), "{message}");
        assert!(message.contains("first-line"), "{message}");
        assert!(message.contains("xxx"), "{message}");
        assert!(!message.contains("TAIL-MARKER"), "{message}");
        assert!(!message.contains('\n'), "{message}");
        assert!(message.chars().count() <= 600, "{message}");
    }

    #[tokio::test]
    async fn malformed_responses_are_errors_not_panics() {
        let bodies = [
            "not json at all",
            "{}",
            r#"{"choices":[]}"#,
            r#"{"choices":"nope"}"#,
            r#"{"choices":[{}]}"#,
            r#"{"choices":[{"message":{}}]}"#,
            r#"{"choices":[{"message":{"content":null}}]}"#,
            r#"{"choices":[{"message":{"content":42}}]}"#,
        ];

        for body in bodies {
            let (address, _requests) = spawn_server(canned(body));
            let error = client(address)
                .complete_json("system", "user", 8, None)
                .await
                .unwrap_err();
            assert!(
                matches!(&error, RuntimeError::BadResponse(_)),
                "{body}: {error}"
            );
        }
    }

    #[tokio::test]
    async fn api_key_is_sent_but_never_debug_printed() {
        let (address, requests) =
            spawn_server(canned(r#"{"choices":[{"message":{"content":"ok"}}]}"#));
        let client = OpenAiClient::new(
            format!("http://{address}/v1"),
            MODEL,
            Some("sk-herald-secret".into()),
        )
        .unwrap();

        let debug = format!("{client:?}");
        assert!(!debug.contains("sk-herald-secret"), "{debug}");
        assert!(debug.contains("REDACTED"), "{debug}");

        client.complete_text("system", "user", 8).await.unwrap();

        let request = requests.recv().unwrap();
        assert_eq!(
            request.header("authorization"),
            Some("Bearer sk-herald-secret")
        );
    }

    #[test]
    fn constructor_rejects_invalid_configuration() {
        assert!(OpenAiClient::new("", MODEL, None).is_err());
        assert!(OpenAiClient::new("   ", MODEL, None).is_err());
        assert!(OpenAiClient::new("ftp://127.0.0.1:11434/v1", MODEL, None).is_err());
        assert!(OpenAiClient::new("file:///tmp/model.gguf", MODEL, None).is_err());
        assert!(OpenAiClient::new("127.0.0.1:11434/v1", MODEL, None).is_err());
        assert!(OpenAiClient::new("http://127.0.0.1:11434/v1", "", None).is_err());
        assert!(OpenAiClient::new("http://127.0.0.1:11434/v1", "   ", None).is_err());
    }

    #[test]
    fn llama_sidecar_implements_the_inference_port() {
        fn assert_inference<T: Inference>() {}

        // Compile-time only: neither adapter is constructed, so no process is
        // spawned and no socket is opened.
        assert_inference::<LlamaSidecar>();
        assert_inference::<OpenAiClient>();
    }
}
