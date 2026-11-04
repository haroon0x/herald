use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use herald_core::{PolicyConfig, ProviderId};
use serde::{Deserialize, Serialize};

use crate::ConfigError;
use crate::paths::{expand_tilde, home_dir};

/// The complete configuration file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Provider used when `--provider` is not given. Must have an adapter.
    pub default_provider: ProviderId,
    pub policy: PolicySettings,
    /// Present only when local email is configured.
    pub maildir: Option<MaildirSettings>,
    pub model: ModelSettings,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_provider: ProviderId::Fake,
            policy: PolicySettings::default(),
            maildir: None,
            model: ModelSettings::default(),
        }
    }
}

impl Config {
    /// Reject configurations that cannot produce a safe, working agent.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !self.default_provider.is_implemented() {
            return Err(ConfigError::Invalid(format!(
                "default_provider = \"{}\" is planned but has no adapter; choose `fake` or `maildir`",
                self.default_provider
            )));
        }

        if self.policy.max_body_chars == 0 || self.policy.max_body_chars > 1_000_000 {
            return Err(ConfigError::Invalid(
                "policy.max_body_chars must be between 1 and 1000000".into(),
            ));
        }

        if self.policy.approval_ttl_hours == 0 || self.policy.approval_ttl_hours > 24 * 30 {
            return Err(ConfigError::Invalid(
                "policy.approval_ttl_hours must be between 1 and 720".into(),
            ));
        }

        if self.default_provider == ProviderId::Maildir && self.maildir.is_none() {
            return Err(ConfigError::Invalid(
                "default_provider = \"maildir\" requires a [maildir] section with a path".into(),
            ));
        }

        if let Some(maildir) = &self.maildir {
            maildir.validate()?;
        }

        self.model.validate()
    }

    /// Apply `HERALD_*` overrides. Injected as a closure so tests do not mutate
    /// process-wide state.
    pub fn apply_env<F>(&mut self, get_env: F) -> Result<(), ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        if let Some(value) = get_env("HERALD_PROVIDER") {
            self.default_provider = ProviderId::from_str(value.trim()).map_err(ConfigError::Env)?;
        }

        if let Some(value) = get_env("HERALD_ALLOW_SEND") {
            self.policy.allow_send = parse_bool("HERALD_ALLOW_SEND", &value)?;
        }

        if let Some(value) = get_env("HERALD_MAILDIR_PATH") {
            let maildir = self.maildir.get_or_insert_with(MaildirSettings::empty);
            maildir.path = PathBuf::from(value);
        }

        if let Some(value) = get_env("HERALD_SMTP_HOST") {
            let maildir = self.maildir.get_or_insert_with(MaildirSettings::empty);
            let smtp = maildir.smtp.get_or_insert_with(SmtpSettings::default);
            smtp.host = value;
        }

        if let Some(value) = get_env("HERALD_SMTP_PORT") {
            let port = value
                .trim()
                .parse::<u16>()
                .map_err(|_| ConfigError::Env("HERALD_SMTP_PORT".into()))?;
            let maildir = self.maildir.get_or_insert_with(MaildirSettings::empty);
            let smtp = maildir.smtp.get_or_insert_with(SmtpSettings::default);
            smtp.port = port;
        }

        if let Some(value) = get_env("HERALD_SMTP_USERNAME") {
            let maildir = self.maildir.get_or_insert_with(MaildirSettings::empty);
            let smtp = maildir.smtp.get_or_insert_with(SmtpSettings::default);
            smtp.username = Some(value);
        }

        if let Some(value) = get_env("HERALD_SMTP_SECURITY") {
            let maildir = self.maildir.get_or_insert_with(MaildirSettings::empty);
            let smtp = maildir.smtp.get_or_insert_with(SmtpSettings::default);
            smtp.security = SmtpSecurity::from_str(value.trim()).map_err(ConfigError::Env)?;
        }

        if let Some(value) = get_env("HERALD_MODEL_BACKEND") {
            self.model.backend = ModelBackend::from_str(value.trim()).map_err(ConfigError::Env)?;
        }

        if let Some(value) = get_env("HERALD_MODEL_ENDPOINT") {
            self.model.endpoint = Some(value);
        }

        if let Some(value) = get_env("HERALD_MODEL_NAME") {
            self.model.model_name = Some(value);
        }

        Ok(())
    }

    /// The policy view used by the domain layer.
    #[must_use]
    pub fn policy(&self) -> PolicyConfig {
        PolicyConfig {
            allow_send: self.policy.allow_send,
            max_body_chars: self.policy.max_body_chars,
        }
    }

    /// Resolved maildir root and Sent folder, if configured.
    #[must_use]
    pub fn maildir_paths(&self) -> Option<(PathBuf, PathBuf)> {
        let maildir = self.maildir.as_ref()?;
        let home = home_dir().unwrap_or_else(|| PathBuf::from("."));
        let root = expand_tilde(&maildir.path, &home);
        let sent = maildir
            .sent_path
            .as_ref()
            .map_or_else(|| root.join(".Sent"), |path| expand_tilde(path, &home));
        Some((root, sent))
    }

    /// Read one dotted configuration key. Returns `None` for unknown keys and
    /// never returns a secret.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<String> {
        self.flat()
            .into_iter()
            .find_map(|(candidate, value)| (candidate == key).then_some(value))
    }

    /// Flatten the configuration into sorted `key = value` pairs for display.
    #[must_use]
    pub fn flat(&self) -> Vec<(String, String)> {
        let mut values = BTreeMap::new();
        values.insert(
            "default_provider".to_string(),
            self.default_provider.as_str().to_string(),
        );
        values.insert(
            "policy.allow_send".to_string(),
            self.policy.allow_send.to_string(),
        );
        values.insert(
            "policy.max_body_chars".to_string(),
            self.policy.max_body_chars.to_string(),
        );
        values.insert(
            "policy.approval_ttl_hours".to_string(),
            self.policy.approval_ttl_hours.to_string(),
        );

        if let Some(maildir) = &self.maildir {
            values.insert(
                "maildir.path".to_string(),
                maildir.path.display().to_string(),
            );
            if let Some(sent) = &maildir.sent_path {
                values.insert("maildir.sent_path".to_string(), sent.display().to_string());
            }
            if let Some(from) = &maildir.from {
                values.insert("maildir.from".to_string(), from.clone());
            }
            if let Some(smtp) = &maildir.smtp {
                values.insert("maildir.smtp.host".to_string(), smtp.host.clone());
                values.insert("maildir.smtp.port".to_string(), smtp.port.to_string());
                values.insert(
                    "maildir.smtp.security".to_string(),
                    smtp.security.as_str().to_string(),
                );
                if let Some(username) = &smtp.username {
                    values.insert("maildir.smtp.username".to_string(), username.clone());
                }
                if let Some(env) = &smtp.password_env {
                    values.insert("maildir.smtp.password_env".to_string(), env.clone());
                }
            }
        }

        values.insert(
            "model.backend".to_string(),
            self.model.backend.as_str().to_string(),
        );
        values.insert(
            "model.context_size".to_string(),
            self.model.context_size.to_string(),
        );
        values.insert("model.threads".to_string(), self.model.threads.to_string());
        if let Some(endpoint) = &self.model.endpoint {
            values.insert("model.endpoint".to_string(), endpoint.clone());
        }
        if let Some(name) = &self.model.model_name {
            values.insert("model.model_name".to_string(), name.clone());
        }
        if let Some(manifest) = &self.model.manifest {
            values.insert("model.manifest".to_string(), manifest.display().to_string());
        }

        values.into_iter().collect()
    }
}

/// Approval and send policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PolicySettings {
    /// Sending is off by default. Enabling it only makes an approval possible;
    /// it never bypasses the approval step.
    pub allow_send: bool,
    pub max_body_chars: usize,
    pub approval_ttl_hours: u64,
}

impl Default for PolicySettings {
    fn default() -> Self {
        Self {
            allow_send: false,
            max_body_chars: 8_000,
            approval_ttl_hours: 72,
        }
    }
}

/// Local mail configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaildirSettings {
    /// INBOX-style Maildir root containing `cur/`, `new/`, and `tmp/`.
    pub path: PathBuf,
    /// Where copies of sent messages are written. Default: `<path>/.Sent`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_path: Option<PathBuf>,
    /// Outbound From header, e.g. `Sam <sam@example.com>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smtp: Option<SmtpSettings>,
}

impl MaildirSettings {
    fn empty() -> Self {
        Self {
            path: PathBuf::new(),
            sent_path: None,
            from: None,
            smtp: None,
        }
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.path.as_os_str().is_empty() {
            return Err(ConfigError::Invalid(
                "maildir.path must not be empty".into(),
            ));
        }
        if let Some(from) = &self.from
            && (!from.contains('@') || from.contains(['\r', '\n']))
        {
            return Err(ConfigError::Invalid(
                "maildir.from must look like `Name <you@example.com>`".into(),
            ));
        }
        if let Some(smtp) = &self.smtp {
            smtp.validate()?;
        }
        Ok(())
    }
}

/// SMTP settings for the Maildir provider. Absent means read-only.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SmtpSettings {
    pub host: String,
    #[serde(default = "default_smtp_port")]
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// Plaintext password. Prefer `password_env`; `doctor` warns about this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_env: Option<String>,
    #[serde(default)]
    pub security: SmtpSecurity,
}

fn default_smtp_port() -> u16 {
    587
}

impl Default for SmtpSettings {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: default_smtp_port(),
            username: None,
            password: None,
            password_env: None,
            security: SmtpSecurity::default(),
        }
    }
}

impl SmtpSettings {
    fn validate(&self) -> Result<(), ConfigError> {
        if self.host.trim().is_empty() || self.host.contains(['\r', '\n', ' ']) {
            return Err(ConfigError::Invalid(
                "maildir.smtp.host must be a hostname without spaces".into(),
            ));
        }
        if self.port == 0 {
            return Err(ConfigError::Invalid(
                "maildir.smtp.port must not be zero".into(),
            ));
        }
        if self.username.is_some() && self.password.is_none() && self.password_env.is_none() {
            return Err(ConfigError::Invalid(
                "maildir.smtp has a username but no password or password_env".into(),
            ));
        }
        Ok(())
    }

    /// Resolve the SMTP password. A direct value wins over the environment;
    /// a configured environment variable that is missing is an error, not a
    /// silent unauthenticated send.
    pub fn resolve_password<F>(&self, get_env: F) -> Result<Option<String>, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        if let Some(password) = &self.password
            && !password.is_empty()
        {
            return Ok(Some(password.clone()));
        }
        if let Some(name) = &self.password_env {
            return match get_env(name) {
                Some(value) if !value.is_empty() => Ok(Some(value)),
                _ => Err(ConfigError::Env(name.clone())),
            };
        }
        Ok(None)
    }

    /// `true` when a password is stored directly in the config file.
    #[must_use]
    pub fn stores_plaintext_password(&self) -> bool {
        self.password
            .as_ref()
            .is_some_and(|value| !value.is_empty())
    }
}

impl fmt::Debug for SmtpSettings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SmtpSettings")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("password_env", &self.password_env)
            .field("security", &self.security)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SmtpSecurity {
    #[default]
    Starttls,
    Tls,
    None,
}

impl SmtpSecurity {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Starttls => "starttls",
            Self::Tls => "tls",
            Self::None => "none",
        }
    }
}

impl FromStr for SmtpSecurity {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "starttls" => Ok(Self::Starttls),
            "tls" => Ok(Self::Tls),
            "none" => Ok(Self::None),
            other => Err(format!(
                "unknown SMTP security {other:?}; expected starttls, tls, or none"
            )),
        }
    }
}

/// Inference backend selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ModelBackend {
    /// Rules and templates only; no model process is started.
    #[default]
    None,
    /// Herald-managed loopback `llama-server` with a pinned GGUF.
    Managed,
    /// Any OpenAI-compatible local endpoint (Ollama, LM Studio, vLLM, ...).
    #[serde(rename = "openai")]
    OpenAi,
}

impl ModelBackend {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Managed => "managed",
            Self::OpenAi => "openai",
        }
    }
}

impl FromStr for ModelBackend {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "none" => Ok(Self::None),
            "managed" => Ok(Self::Managed),
            "openai" => Ok(Self::OpenAi),
            other => Err(format!(
                "unknown model backend {other:?}; expected none, managed, or openai"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModelSettings {
    pub backend: ModelBackend,
    /// OpenAI-compatible base URL, e.g. `http://127.0.0.1:11434/v1`.
    pub endpoint: Option<String>,
    pub model_name: Option<String>,
    /// Path to a model manifest (managed backend only).
    pub manifest: Option<PathBuf>,
    /// Environment variable holding an optional bearer token.
    pub api_key_env: Option<String>,
    pub context_size: u32,
    pub threads: u16,
}

impl Default for ModelSettings {
    fn default() -> Self {
        Self {
            backend: ModelBackend::None,
            endpoint: None,
            model_name: None,
            manifest: None,
            api_key_env: None,
            context_size: 4_096,
            threads: 4,
        }
    }
}

impl ModelSettings {
    fn validate(&self) -> Result<(), ConfigError> {
        match self.backend {
            ModelBackend::None => {}
            ModelBackend::Managed => {
                if self.context_size < 512 {
                    return Err(ConfigError::Invalid(
                        "model.context_size must be at least 512".into(),
                    ));
                }
                if self.threads == 0 {
                    return Err(ConfigError::Invalid(
                        "model.threads must be at least 1".into(),
                    ));
                }
            }
            ModelBackend::OpenAi => {
                let endpoint = self.endpoint.as_deref().unwrap_or_default().trim();
                if endpoint.is_empty() {
                    return Err(ConfigError::Invalid(
                        "model.endpoint is required for the openai backend".into(),
                    ));
                }
                if !(endpoint.starts_with("http://") || endpoint.starts_with("https://")) {
                    return Err(ConfigError::Invalid(
                        "model.endpoint must be an http:// or https:// URL".into(),
                    ));
                }
                if self
                    .model_name
                    .as_deref()
                    .unwrap_or_default()
                    .trim()
                    .is_empty()
                {
                    return Err(ConfigError::Invalid(
                        "model.model_name is required for the openai backend".into(),
                    ));
                }
            }
        }
        Ok(())
    }
}

fn parse_bool(name: &str, value: &str) -> Result<bool, ConfigError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(ConfigError::Env(name.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bool_parsing_is_strict() {
        assert!(parse_bool("X", "yes").unwrap());
        assert!(!parse_bool("X", "off").unwrap());
        assert!(parse_bool("X", "maybe").is_err());
    }

    #[test]
    fn manual_debug_redacts_the_password() {
        let smtp = SmtpSettings {
            host: "smtp.example.com".into(),
            username: Some("me".into()),
            password: Some("hunter2".into()),
            ..SmtpSettings::default()
        };
        let rendered = format!("{smtp:?}");
        assert!(!rendered.contains("hunter2"));
        assert!(rendered.contains("redacted"));
    }
}
