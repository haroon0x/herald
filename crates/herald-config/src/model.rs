//! The `config.toml` model: parsing, defaults, validation, environment overrides,
//! and secret redaction. The CLI never parses TOML itself.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
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
    /// Present only when IMAP mail is configured.
    pub imap: Option<ImapSettings>,
    /// Present only when the X (Twitter) browser adapter is configured.
    pub x: Option<XSettings>,
    pub model: ModelSettings,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_provider: ProviderId::Fake,
            policy: PolicySettings::default(),
            maildir: None,
            imap: None,
            x: None,
            model: ModelSettings::default(),
        }
    }
}

impl Config {
    /// Reject configurations that cannot produce a safe, working agent.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !self.default_provider.is_implemented() {
            return Err(ConfigError::Invalid(format!(
                "default_provider = \"{}\" is planned but has no adapter; choose `fake`, `maildir`, `imap`, or `x`",
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

        if let Some(imap) = &self.imap {
            imap.validate()?;
        }

        if let Some(x) = &self.x {
            validate_x_handle(x.handle.as_deref())?;
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

        self.apply_imap_env(&get_env)?;
        self.apply_x_env(&get_env)?;

        Ok(())
    }

    /// `HERALD_X_*` overrides, split out to keep `apply_env` readable.
    fn apply_x_env<F>(&mut self, get_env: &F) -> Result<(), ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        if let Some(value) = get_env("HERALD_X_PROFILE_DIR") {
            let x = self.x.get_or_insert_with(XSettings::empty);
            x.profile_dir = Some(PathBuf::from(value));
        }

        if let Some(value) = get_env("HERALD_X_HANDLE") {
            let x = self.x.get_or_insert_with(XSettings::empty);
            x.handle = Some(value);
        }

        if let Some(value) = get_env("HERALD_X_HEADLESS") {
            let x = self.x.get_or_insert_with(XSettings::empty);
            x.headless = parse_bool("HERALD_X_HEADLESS", &value)?;
        }

        if let Some(value) = get_env("HERALD_X_ACTION_DELAY_MS") {
            let delay = value
                .trim()
                .parse::<u64>()
                .map_err(|_| ConfigError::Env("HERALD_X_ACTION_DELAY_MS".into()))?;
            let x = self.x.get_or_insert_with(XSettings::empty);
            x.action_delay_ms = delay;
        }

        if let Some(value) = get_env("HERALD_X_TYPING_DELAY_MS") {
            let delay = value
                .trim()
                .parse::<u64>()
                .map_err(|_| ConfigError::Env("HERALD_X_TYPING_DELAY_MS".into()))?;
            let x = self.x.get_or_insert_with(XSettings::empty);
            x.typing_delay_ms = delay;
        }

        if let Some(value) = get_env("HERALD_X_CLOSE_AFTER") {
            let x = self.x.get_or_insert_with(XSettings::empty);
            x.close_after = parse_bool("HERALD_X_CLOSE_AFTER", &value)?;
        }

        if let Some(value) = get_env("HERALD_X_READ_ONLY") {
            let x = self.x.get_or_insert_with(XSettings::empty);
            x.read_only = parse_bool("HERALD_X_READ_ONLY", &value)?;
        }

        Ok(())
    }

    /// `HERALD_IMAP_*` overrides, split out like `apply_x_env`.
    fn apply_imap_env<F>(&mut self, get_env: &F) -> Result<(), ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        if let Some(value) = get_env("HERALD_IMAP_HOST") {
            let imap = self.imap.get_or_insert_with(ImapSettings::empty);
            imap.host = value;
        }

        if let Some(value) = get_env("HERALD_IMAP_PORT") {
            let port = value
                .trim()
                .parse::<u16>()
                .map_err(|_| ConfigError::Env("HERALD_IMAP_PORT".into()))?;
            let imap = self.imap.get_or_insert_with(ImapSettings::empty);
            imap.port = port;
        }

        if let Some(value) = get_env("HERALD_IMAP_USERNAME") {
            let imap = self.imap.get_or_insert_with(ImapSettings::empty);
            imap.username = Some(value);
        }

        if let Some(value) = get_env("HERALD_IMAP_FOLDER") {
            let imap = self.imap.get_or_insert_with(ImapSettings::empty);
            imap.folder = value;
        }

        if let Some(value) = get_env("HERALD_IMAP_TLS") {
            let imap = self.imap.get_or_insert_with(ImapSettings::empty);
            imap.use_tls = parse_bool("HERALD_IMAP_TLS", &value)?;
        }

        if let Some(value) = get_env("HERALD_IMAP_MARK_READ_ON_OPEN") {
            let imap = self.imap.get_or_insert_with(ImapSettings::empty);
            imap.mark_read_on_open = parse_bool("HERALD_IMAP_MARK_READ_ON_OPEN", &value)?;
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

    /// Resolve the X browser profile directory: configured value, expanded via
    /// `expand_tilde` (like `maildir_paths`), or `<data_dir>/profiles/x`.
    #[must_use]
    pub fn x_profile_dir(&self, data_dir: &Path) -> PathBuf {
        match self.x.as_ref().and_then(|x| x.profile_dir.as_ref()) {
            Some(profile_dir) => {
                let home = home_dir().unwrap_or_else(|| PathBuf::from("."));
                expand_tilde(profile_dir, &home)
            }
            None => data_dir.join("profiles").join("x"),
        }
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
        if let Some(imap) = &self.imap {
            values.insert("imap.host".to_string(), imap.host.clone());
            values.insert("imap.port".to_string(), imap.port.to_string());
            if let Some(username) = &imap.username {
                values.insert("imap.username".to_string(), username.clone());
            }
            if let Some(env) = &imap.password_env {
                values.insert("imap.password_env".to_string(), env.clone());
            }
            values.insert("imap.folder".to_string(), imap.folder.clone());
            values.insert("imap.use_tls".to_string(), imap.use_tls.to_string());
            values.insert(
                "imap.mark_read_on_open".to_string(),
                imap.mark_read_on_open.to_string(),
            );
        }

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

        if let Some(x) = &self.x {
            if let Some(profile_dir) = &x.profile_dir {
                values.insert(
                    "x.profile_dir".to_string(),
                    profile_dir.display().to_string(),
                );
            }
            if let Some(handle) = &x.handle {
                values.insert("x.handle".to_string(), handle.clone());
            }
            values.insert("x.headless".to_string(), x.headless.to_string());
            values.insert(
                "x.action_delay_ms".to_string(),
                x.action_delay_ms.to_string(),
            );
            values.insert(
                "x.typing_delay_ms".to_string(),
                x.typing_delay_ms.to_string(),
            );
            values.insert("x.close_after".to_string(), x.close_after.to_string());
            values.insert("x.read_only".to_string(), x.read_only.to_string());
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

/// IMAP mail configuration. Read-only; sending is not part of v1.1.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ImapSettings {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    /// Plaintext password. Prefer `password_env`; `doctor` warns about this.
    pub password: Option<String>,
    pub password_env: Option<String>,
    pub folder: String,
    pub use_tls: bool,
    /// Opening a thread marks it read on the server. Default true, mirroring
    /// the X adapter's local seen-ledger; set false to keep reads side-effect
    /// free.
    pub mark_read_on_open: bool,
}

impl Default for ImapSettings {
    fn default() -> Self {
        Self {
            host: "imap.gmail.com".to_string(),
            port: 993,
            username: None,
            password: None,
            password_env: None,
            folder: "INBOX".to_string(),
            use_tls: true,
            mark_read_on_open: true,
        }
    }
}

impl ImapSettings {
    fn empty() -> Self {
        Self {
            host: String::new(),
            port: 993,
            username: None,
            password: None,
            password_env: None,
            folder: "INBOX".to_string(),
            use_tls: true,
            mark_read_on_open: true,
        }
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.host.trim().is_empty() || self.host.contains(['\r', '\n', ' ']) {
            return Err(ConfigError::Invalid(
                "imap.host must be a hostname without spaces".into(),
            ));
        }
        if self.port == 0 {
            return Err(ConfigError::Invalid("imap.port must not be zero".into()));
        }
        if self.folder.trim().is_empty() {
            return Err(ConfigError::Invalid("imap.folder must not be empty".into()));
        }
        if self.username.is_some() && self.password.is_none() && self.password_env.is_none() {
            return Err(ConfigError::Invalid(
                "imap has a username but no password or password_env".into(),
            ));
        }
        Ok(())
    }

    /// Resolve the password. A direct value wins over the environment; a
    /// configured environment variable that is missing is an error, not a
    /// silent attempt to log in without credentials.
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

impl fmt::Debug for ImapSettings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ImapSettings")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("password_env", &self.password_env)
            .field("folder", &self.folder)
            .field("use_tls", &self.use_tls)
            .field("mark_read_on_open", &self.mark_read_on_open)
            .finish()
    }
}

/// Validate an optional X handle: no whitespace/newlines, at most 16 chars
/// after an optional leading '@'.
fn validate_x_handle(handle: Option<&str>) -> Result<(), ConfigError> {
    let Some(handle) = handle else {
        return Ok(());
    };
    if handle.contains(['\r', '\n', ' ']) {
        return Err(ConfigError::Invalid(
            "x.handle must not contain whitespace".into(),
        ));
    }
    let stripped = handle.strip_prefix('@').unwrap_or(handle);
    if stripped.len() > 16 {
        return Err(ConfigError::Invalid(
            "x.handle must be at most 16 characters (after an optional '@')".into(),
        ));
    }
    Ok(())
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

/// X (Twitter) browser adapter configuration.
///
/// The profile directory is dedicated: it must never point at an everyday
/// Chrome profile. Login is done once by the user in that profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct XSettings {
    /// Dedicated browser profile directory. `None` resolves under the data dir.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_dir: Option<PathBuf>,
    /// Display-only handle, e.g. `@someone` or `someone`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    pub headless: bool,
    pub action_delay_ms: u64,
    pub typing_delay_ms: u64,
    pub close_after: bool,
    /// Refuse every X write; reads keep working.
    pub read_only: bool,
}

impl Default for XSettings {
    fn default() -> Self {
        Self {
            profile_dir: None,
            handle: None,
            headless: true,
            action_delay_ms: 1_800,
            typing_delay_ms: 20,
            close_after: true,
            read_only: false,
        }
    }
}

impl XSettings {
    fn empty() -> Self {
        Self {
            profile_dir: None,
            handle: None,
            headless: true,
            action_delay_ms: 1_800,
            typing_delay_ms: 20,
            close_after: true,
            read_only: false,
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
    /// Local `opencode` CLI free-model pool (`opencode run --model ...`).
    #[serde(rename = "opencode")]
    OpenCode,
}

impl ModelBackend {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Managed => "managed",
            Self::OpenAi => "openai",
            Self::OpenCode => "opencode",
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
            "opencode" => Ok(Self::OpenCode),
            other => Err(format!(
                "unknown model backend {other:?}; expected none, managed, openai, or opencode"
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
            // The free pool is a named model id; a missing value gets the
            // calibrated default in herald-runtime, so nothing to check here.
            ModelBackend::None | ModelBackend::OpenCode => {}
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

    #[test]
    fn imap_defaults_target_gmail_tls() {
        let settings = ImapSettings::default();
        assert_eq!(settings.host, "imap.gmail.com");
        assert_eq!(settings.port, 993);
        assert_eq!(settings.folder, "INBOX");
        assert!(settings.use_tls);
        assert!(settings.mark_read_on_open);
        assert!(Config::default().imap.is_none());
    }

    #[test]
    fn imap_username_requires_a_password_source() {
        let mut config = Config {
            imap: Some(ImapSettings {
                username: Some("me@gmail.com".into()),
                ..ImapSettings::default()
            }),
            ..Config::default()
        };
        assert!(config.validate().is_err());
        config.imap.as_mut().unwrap().password_env = Some("HERALD_IMAP_PASSWORD".into());
        config.validate().unwrap();
    }

    #[test]
    fn imap_password_resolves_like_smtp() {
        let settings = ImapSettings {
            password: Some("hunter2".into()),
            password_env: Some("HERALD_IMAP_PASSWORD".into()),
            ..ImapSettings::default()
        };
        assert_eq!(
            settings.resolve_password(|_| None).unwrap().as_deref(),
            Some("hunter2")
        );

        let settings = ImapSettings {
            password_env: Some("HERALD_IMAP_PASSWORD".into()),
            ..ImapSettings::default()
        };
        assert!(settings.resolve_password(|_| None).is_err());
        assert_eq!(
            settings
                .resolve_password(|key| (key == "HERALD_IMAP_PASSWORD").then(|| "secret".into()))
                .unwrap()
                .as_deref(),
            Some("secret")
        );
        let rendered = format!("{settings:?}");
        assert!(!rendered.contains("secret"));
    }

    #[test]
    fn imap_env_overrides_apply() {
        let mut config = Config::default();
        config
            .apply_env(|key| match key {
                "HERALD_IMAP_HOST" => Some("imap.example.com".into()),
                "HERALD_IMAP_PORT" => Some("143".into()),
                "HERALD_IMAP_USERNAME" => Some("me@example.com".into()),
                "HERALD_IMAP_FOLDER" => Some("Archive".into()),
                "HERALD_IMAP_TLS" => Some("off".into()),
                "HERALD_IMAP_MARK_READ_ON_OPEN" => Some("0".into()),
                _ => None,
            })
            .unwrap();
        let imap = config.imap.unwrap();
        assert_eq!(imap.host, "imap.example.com");
        assert_eq!(imap.port, 143);
        assert_eq!(imap.username.as_deref(), Some("me@example.com"));
        assert_eq!(imap.folder, "Archive");
        assert!(!imap.use_tls);
        assert!(!imap.mark_read_on_open);
    }

    #[test]
    fn flat_contains_imap_keys_only_when_set() {
        let config = Config::default();
        assert!(!config.flat().iter().any(|(k, _)| k.starts_with("imap.")));
        let config = Config {
            imap: Some(ImapSettings::default()),
            ..Config::default()
        };
        let flat = config.flat();
        for key in [
            "imap.host",
            "imap.port",
            "imap.folder",
            "imap.use_tls",
            "imap.mark_read_on_open",
        ] {
            assert!(flat.iter().any(|(k, _)| k == key), "missing {key}");
        }
    }

    #[test]
    fn xsettings_serde_defaults() {
        let config: Config = toml::from_str("[x]\n").unwrap();
        let x = config.x.unwrap();
        assert_eq!(x.profile_dir, None);
        assert_eq!(x.handle, None);
        assert!(x.headless);
        assert_eq!(x.action_delay_ms, 1_800);
        assert_eq!(x.typing_delay_ms, 20);
        assert!(x.close_after);
    }

    #[test]
    fn x_handle_validation_rejects_bad_handles() {
        for bad in [
            "has space",
            "bad\nnewline",
            "bad\rnewline",
            "waytoolonghandle123",
        ] {
            let config = Config {
                x: Some(XSettings {
                    handle: Some(bad.into()),
                    ..XSettings::default()
                }),
                ..Config::default()
            };
            assert!(
                config.validate().is_err(),
                "handle {bad:?} should be rejected"
            );
        }

        for good in ["@someone", "someone", "@abcdefghijklmnop"] {
            let config = Config {
                x: Some(XSettings {
                    handle: Some(good.into()),
                    ..XSettings::default()
                }),
                ..Config::default()
            };
            config.validate().unwrap();
        }
    }

    #[test]
    fn x_profile_dir_resolution() {
        let config = Config::default();
        assert_eq!(
            config.x_profile_dir(Path::new("/data")),
            PathBuf::from("/data/profiles/x")
        );

        let config = Config {
            x: Some(XSettings {
                profile_dir: Some(PathBuf::from("/tmp/custom-x")),
                ..XSettings::default()
            }),
            ..Config::default()
        };
        assert_eq!(
            config.x_profile_dir(Path::new("/data")),
            PathBuf::from("/tmp/custom-x")
        );
    }

    #[test]
    fn flat_contains_x_keys_only_when_set() {
        let config = Config::default();
        assert!(!config.flat().iter().any(|(k, _)| k.starts_with("x.")));

        let config = Config {
            x: Some(XSettings {
                profile_dir: Some(PathBuf::from("/tmp/x")),
                handle: Some("@someone".into()),
                ..XSettings::default()
            }),
            ..Config::default()
        };
        let flat = config.flat();
        for key in [
            "x.profile_dir",
            "x.handle",
            "x.headless",
            "x.action_delay_ms",
            "x.typing_delay_ms",
            "x.close_after",
            "x.read_only",
        ] {
            assert!(flat.iter().any(|(k, _)| k == key), "missing flat key {key}");
        }
    }

    #[test]
    fn stale_autopost_config_keys_fail_closed() {
        // The autopost lane was removed. A config file that still carries
        // its keys is rejected outright — Herald never starts with a stale
        // autonomy flag it no longer understands. The error names the key.
        for key in [
            "allow_autopost = true",
            "autopost_daily_limit = 5",
            "autopost_min_quality = 7",
            "autopost_quiet_hours = \"22:00-06:00\"",
        ] {
            let text = format!("[policy]\n{key}\n");
            let error = crate::parse(&text).unwrap_err();
            assert!(
                matches!(error, crate::ConfigError::Parse(_)),
                "{key} should be rejected"
            );
        }
    }

    #[test]
    fn x_env_overrides_apply() {
        let mut config = Config::default();
        config
            .apply_env(|key| match key {
                "HERALD_X_PROFILE_DIR" => Some("/tmp/x".into()),
                "HERALD_X_HANDLE" => Some("@someone".into()),
                "HERALD_X_HEADLESS" => Some("false".into()),
                "HERALD_X_ACTION_DELAY_MS" => Some("500".into()),
                "HERALD_X_TYPING_DELAY_MS" => Some("5".into()),
                "HERALD_X_CLOSE_AFTER" => Some("off".into()),
                _ => None,
            })
            .unwrap();
        let x = config.x.unwrap();
        assert_eq!(x.profile_dir, Some(PathBuf::from("/tmp/x")));
        assert_eq!(x.handle.as_deref(), Some("@someone"));
        assert!(!x.headless);
        assert_eq!(x.action_delay_ms, 500);
        assert_eq!(x.typing_delay_ms, 5);
        assert!(!x.close_after);
    }
}
