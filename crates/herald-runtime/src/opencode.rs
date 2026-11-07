//! Local `opencode` CLI backend for the free-model pool.
//!
//! Herald runs `opencode run --standalone --model <id> <prompt>` as a one-shot
//! subprocess with the current directory set to an empty sandbox under the
//! data dir. `--auto` is never passed: the CLI must not approve its own tool
//! calls. The model's stdout is treated as text, sliced to one JSON object, and
//! validated by the caller against the planner schema exactly like any other
//! backend.
//!
//! This is a local process on the user's machine, but it is still a model: it
//! emits plans, never actions. Nothing here executes provider code.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;

use crate::inference::Inference;
use crate::{RuntimeError, inference::slice_json_object};

/// Default one-shot budget, matching the other backends.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
/// Model used when the configuration does not name one.
pub const DEFAULT_MODEL: &str = "opencode/glm-5.3-flash-free";
/// How much of a failing stderr is echoed in the error message.
const MAX_ERROR_TAIL_CHARS: usize = 400;

/// The free-model opencode CLI, one subprocess per completion.
#[derive(Debug, Clone)]
pub struct OpenCodeInference {
    binary: PathBuf,
    model: String,
    sandbox_dir: PathBuf,
    timeout: Duration,
}

impl OpenCodeInference {
    /// Build a backend. `sandbox_dir` is created on first use and is the
    /// subprocess's working directory; nothing in it is read back.
    #[must_use]
    pub fn new(binary: PathBuf, model: impl Into<String>, sandbox_dir: PathBuf) -> Self {
        let model = model.into();
        let model = model.trim();
        let model = if model.is_empty() {
            DEFAULT_MODEL.to_string()
        } else {
            model.to_string()
        };
        Self {
            binary,
            model,
            sandbox_dir,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Override the one-shot timeout. Tests use a short value.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }

    #[must_use]
    pub fn sandbox_dir(&self) -> &Path {
        &self.sandbox_dir
    }

    async fn run(&self, prompt: &str) -> Result<String, RuntimeError> {
        std::fs::create_dir_all(&self.sandbox_dir)?;

        let mut command = tokio::process::Command::new(&self.binary);
        command
            .arg("run")
            .arg("--standalone")
            .arg("--model")
            .arg(&self.model)
            .arg(prompt)
            .current_dir(&self.sandbox_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let output = tokio::time::timeout(self.timeout, command.output())
            .await
            .map_err(|_| {
                RuntimeError::ModelProcess(format!(
                    "opencode did not answer within {}s",
                    self.timeout.as_secs()
                ))
            })?
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    RuntimeError::BinaryMissing(self.binary.clone())
                } else {
                    RuntimeError::Io(error)
                }
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(RuntimeError::ModelProcess(format!(
                "opencode exited with {}: {}",
                output.status,
                tail(&stderr, MAX_ERROR_TAIL_CHARS)
            )));
        }

        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

#[async_trait]
impl Inference for OpenCodeInference {
    async fn complete_json(
        &self,
        system: &str,
        user: &str,
        _max_tokens: u16,
        _schema: Option<&Value>,
    ) -> Result<String, RuntimeError> {
        let prompt = format!("{system}\n\n{user}\n\nReply with one JSON object and nothing else.");
        let raw = self.run(&prompt).await?;
        let stripped = strip_opencode_banner(&raw);
        let json = slice_json_object(&stripped).ok_or_else(|| {
            RuntimeError::ModelProcess("opencode did not return a JSON object".into())
        })?;
        serde_json::from_str::<Value>(json).map_err(|error| {
            RuntimeError::ModelProcess(format!("opencode returned invalid JSON: {error}"))
        })?;
        Ok(json.to_string())
    }

    async fn complete_text(
        &self,
        system: &str,
        user: &str,
        _max_tokens: u16,
    ) -> Result<String, RuntimeError> {
        let prompt = format!("{system}\n\n{user}");
        let raw = self.run(&prompt).await?;
        let text = strip_opencode_banner(&raw);
        if text.trim().is_empty() {
            return Err(RuntimeError::ModelProcess(
                "opencode returned no text".to_string(),
            ));
        }
        Ok(text)
    }
}

/// Drop the `> build · <model>` banner opencode prints first.
///
/// Only a leading block of `>`-prefixed lines is removed, so a model answer
/// that happens to quote a prompt is not touched.
#[must_use]
fn strip_opencode_banner(raw: &str) -> String {
    let mut lines = raw.lines().peekable();
    while lines
        .peek()
        .is_some_and(|line| line.trim_start().starts_with('>'))
    {
        lines.next();
    }
    let mut rest: String = lines.collect::<Vec<_>>().join("\n");
    while rest.starts_with('\n') {
        rest.remove(0);
    }
    rest.trim_end().to_string()
}

fn tail(value: &str, limit: usize) -> String {
    let trimmed = value.trim();
    let chars: Vec<char> = trimmed.chars().collect();
    if chars.len() <= limit {
        return trimmed.to_string();
    }
    chars[chars.len() - limit..].iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_is_stripped_but_quoted_prompts_are_not() {
        assert_eq!(
            strip_opencode_banner("> build · glm-5.3-flash-free\nok"),
            "ok"
        );
        assert_eq!(
            strip_opencode_banner("> build · model\n> thinking\n{\"a\":1}"),
            "{\"a\":1}"
        );
        assert_eq!(
            strip_opencode_banner("The model said:\n> quote\n"),
            "The model said:\n> quote"
        );
    }

    #[test]
    fn json_object_is_sliced_out_of_chatter() {
        assert_eq!(
            slice_json_object("Sure! {\"operation\":\"get_thread\"} thanks"),
            Some("{\"operation\":\"get_thread\"}")
        );
        assert_eq!(slice_json_object("no json here"), None);
        assert_eq!(slice_json_object("}{"), None);
    }

    #[test]
    fn error_tail_is_bounded_and_keeps_the_end() {
        let long = "x".repeat(1_000);
        let kept = tail(&long, 100);
        assert_eq!(kept.len(), 100);
    }

    #[tokio::test]
    async fn a_missing_binary_is_reported_as_missing() {
        let backend = OpenCodeInference::new(
            PathBuf::from("/nonexistent/opencode-herald-test"),
            DEFAULT_MODEL,
            std::env::temp_dir().join("herald-opencode-missing"),
        );
        assert!(matches!(
            backend.complete_text("system", "user", 64).await,
            Err(RuntimeError::BinaryMissing(_))
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_invocation_never_passes_auto_and_parses_json() {
        use std::io::Write as _;
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("args.txt");
        let script = dir.path().join("fake-opencode");
        let mut file = std::fs::File::create(&script).unwrap();
        writeln!(
            file,
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\necho '> build · fake'\necho '{{\"operation\":\"get_thread\"}}'",
            log.display()
        )
        .unwrap();
        drop(file);
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let backend =
            OpenCodeInference::new(script, "opencode/test-model", dir.path().join("sandbox"));
        let json = backend
            .complete_json("system", "user", 128, None)
            .await
            .unwrap();
        assert_eq!(json, "{\"operation\":\"get_thread\"}");

        let args = std::fs::read_to_string(&log).unwrap();
        let args: Vec<&str> = args.lines().collect();
        assert_eq!(args[0], "run");
        assert_eq!(args[1], "--standalone");
        assert_eq!(args[2], "--model");
        assert_eq!(args[3], "opencode/test-model");
        assert!(!args.contains(&"--auto"));
        assert!(backend.sandbox_dir().exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_non_zero_exit_surfaces_the_stderr_tail() {
        use std::io::Write as _;
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("failing-opencode");
        let mut file = std::fs::File::create(&script).unwrap();
        writeln!(
            file,
            "#!/bin/sh\necho 'free usage limit reached' >&2\nexit 1"
        )
        .unwrap();
        drop(file);
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let backend = OpenCodeInference::new(script, DEFAULT_MODEL, dir.path().join("sandbox"));
        match backend.complete_text("system", "user", 64).await {
            Err(RuntimeError::ModelProcess(message)) => {
                assert!(message.contains("free usage limit reached"));
            }
            other => panic!("expected ModelProcess, got {other:?}"),
        }
    }
}
