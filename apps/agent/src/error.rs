//! Structured error reporting and exit codes.
//!
//! Exit codes are part of the interface:
//! 0 success, 1 runtime failure, 2 usage, 3 policy rejection,
//! 4 stale/changed approval, 5 eval threshold failure.

use serde_json::json;

use crate::output;

#[derive(Debug, Clone)]
pub struct AppError {
    pub code: &'static str,
    pub message: String,
    pub hint: Option<String>,
    pub exit: u8,
}

impl std::fmt::Display for AppError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl AppError {
    #[must_use]
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            hint: None,
            exit: 1,
        }
    }

    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    #[must_use]
    pub fn policy(message: impl Into<String>) -> Self {
        Self {
            code: "policy_rejected",
            message: message.into(),
            hint: None,
            exit: 3,
        }
    }

    #[must_use]
    pub fn conflict(message: impl Into<String>) -> Self {
        Self {
            code: "approval_conflict",
            message: message.into(),
            hint: Some(
                "Nothing was sent. Plan the request again to create a fresh approval.".into(),
            ),
            exit: 4,
        }
    }

    #[must_use]
    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            code: "not_found",
            message: message.into(),
            hint: None,
            exit: 1,
        }
    }

    #[must_use]
    pub const fn exit_code(&self) -> u8 {
        self.exit
    }

    /// Emit the error in the selected format. JSON goes to stdout so a caller
    /// can parse one object; human errors go to stderr.
    pub fn report(&self) {
        if output::json_mode() {
            println!(
                "{}",
                json!({
                    "ok": false,
                    "error": {
                        "code": self.code,
                        "message": self.message,
                        "hint": self.hint,
                    }
                })
            );
        } else {
            eprintln!("error: {}", self.message);
            if let Some(hint) = &self.hint {
                eprintln!("hint:  {hint}");
            }
        }
    }
}

impl From<anyhow::Error> for AppError {
    fn from(error: anyhow::Error) -> Self {
        Self::new("internal", error.to_string())
    }
}

impl From<herald_storage::StorageError> for AppError {
    fn from(error: herald_storage::StorageError) -> Self {
        Self::new("storage", error.to_string())
    }
}

impl From<herald_config::ConfigError> for AppError {
    fn from(error: herald_config::ConfigError) -> Self {
        Self::new("config", error.to_string())
    }
}

impl From<herald_provider::ProviderError> for AppError {
    fn from(error: herald_provider::ProviderError) -> Self {
        use herald_provider::ProviderError;
        let code = match error {
            ProviderError::NotFound => "not_found",
            ProviderError::NotAuthenticated => "not_authenticated",
            ProviderError::NotConfigured(_) => "provider_not_configured",
            ProviderError::Timeout => "provider_timeout",
            ProviderError::RateLimited => "rate_limited",
            ProviderError::UnexpectedLayout(_) => "provider_layout",
            ProviderError::Unsupported => "unsupported",
            ProviderError::Other(_) => "provider",
        };
        Self::new(code, error.to_string())
    }
}

impl From<herald_runtime::RuntimeError> for AppError {
    fn from(error: herald_runtime::RuntimeError) -> Self {
        Self::new("model", error.to_string())
    }
}
