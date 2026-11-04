//! Typed configuration for Herald.
//!
//! Configuration is deliberately boring: one TOML file, a fixed set of keys,
//! unknown keys rejected, and environment overrides that are documented in
//! `docs/configuration.md`. Secrets are never stored in the file by `init`;
//! they are resolved from the environment at the moment they are needed.

mod model;
mod paths;
mod template;

use std::path::{Path, PathBuf};

pub use model::{
    Config, MaildirSettings, ModelBackend, ModelSettings, PolicySettings, SmtpSecurity,
    SmtpSettings,
};
pub use paths::{default_config_path, default_data_dir, ensure_private_dir, expand_tilde};
pub use template::TEMPLATE;

/// Why a configuration could not be loaded or used.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("configuration file not found: {0}")]
    NotFound(PathBuf),
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid TOML configuration: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("invalid configuration: {0}")]
    Invalid(String),
    #[error("environment variable {0} is invalid")]
    Env(String),
}

/// Where the effective configuration came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigSource {
    /// No file existed; built-in defaults are in use.
    Defaults,
    File(PathBuf),
}

/// The loaded configuration plus its provenance.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub config: Config,
    pub source: ConfigSource,
}

impl Loaded {
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        match &self.source {
            ConfigSource::Defaults => None,
            ConfigSource::File(path) => Some(path),
        }
    }
}

/// Load configuration using the standard precedence:
/// explicit path, `HERALD_CONFIG`, platform config dir, built-in defaults.
///
/// Environment overrides (`HERALD_PROVIDER`, `HERALD_ALLOW_SEND`, ...) are
/// applied after the file. CLI flags are applied by the caller afterwards.
pub fn load(explicit: Option<&Path>) -> Result<Loaded, ConfigError> {
    let path = resolve_path(explicit)?;

    let mut config = match &path {
        Some(path) => {
            let text = std::fs::read_to_string(path).map_err(|error| ConfigError::Io {
                path: path.clone(),
                source: error,
            })?;
            toml::from_str(&text)?
        }
        None => Config::default(),
    };

    config.apply_env(|key| std::env::var(key).ok())?;
    config.validate()?;

    Ok(Loaded {
        config,
        source: path.map_or(ConfigSource::Defaults, ConfigSource::File),
    })
}

/// Parse a configuration string without touching the filesystem or environment.
pub fn parse(text: &str) -> Result<Config, ConfigError> {
    let config: Config = toml::from_str(text)?;
    config.validate()?;
    Ok(config)
}

/// Load and merge environment overrides, then validate.
pub fn parse_with_env<F>(text: &str, get_env: F) -> Result<Config, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let mut config: Config = toml::from_str(text)?;
    config.apply_env(get_env)?;
    config.validate()?;
    Ok(config)
}

/// Resolve the configuration path, distinguishing "not configured" from
/// "explicitly configured but missing".
fn resolve_path(explicit: Option<&Path>) -> Result<Option<PathBuf>, ConfigError> {
    if let Some(path) = explicit {
        return if path.exists() {
            Ok(Some(path.to_path_buf()))
        } else {
            Err(ConfigError::NotFound(path.to_path_buf()))
        };
    }

    if let Some(raw) = std::env::var_os("HERALD_CONFIG") {
        let path = PathBuf::from(raw);
        return if path.exists() {
            Ok(Some(path))
        } else {
            Err(ConfigError::NotFound(path))
        };
    }

    let path = default_config_path();
    Ok(path.exists().then_some(path))
}

/// Resolve the data directory: explicit flag, `HERALD_DATA_DIR`, platform dir.
pub fn resolve_data_dir(cli: Option<&Path>) -> Result<PathBuf, ConfigError> {
    if let Some(path) = cli {
        return Ok(path.to_path_buf());
    }
    if let Some(raw) = std::env::var_os("HERALD_DATA_DIR") {
        let path = PathBuf::from(raw);
        if path.as_os_str().is_empty() {
            return Err(ConfigError::Invalid(
                "HERALD_DATA_DIR is set but empty".into(),
            ));
        }
        return Ok(path);
    }
    default_data_dir()
        .ok_or_else(|| ConfigError::Invalid("could not determine a platform data directory".into()))
}

/// Write the annotated starter configuration. Refuses to overwrite unless
/// `force` is set.
pub fn write_template(path: &Path, force: bool) -> Result<(), ConfigError> {
    if path.exists() && !force {
        return Err(ConfigError::Invalid(format!(
            "{} already exists; pass --force to overwrite",
            path.display()
        )));
    }
    if let Some(parent) = path.parent() {
        ensure_private_dir(parent)?;
    }
    std::fs::write(path, TEMPLATE).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    restrict_file(path)
}

/// Warn when a configuration containing credentials is readable by other users.
#[must_use]
pub fn permission_warning(path: &Path) -> Option<String> {
    #[cfg(not(unix))]
    let _ = path;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path).ok()?.permissions().mode();
        if mode & 0o077 != 0 {
            return Some(format!(
                "{} is readable by group/other (mode {:o}); run `chmod 600 {}`",
                path.display(),
                mode & 0o777,
                path.display()
            ));
        }
    }
    None
}

fn restrict_file(path: &Path) -> Result<(), ConfigError> {
    #[cfg(not(unix))]
    let _ = path;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(path)
            .map_err(|source| ConfigError::Io {
                path: path.to_path_buf(),
                source,
            })?
            .permissions();
        permissions.set_mode(0o600);
        std::fs::set_permissions(path, permissions).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use herald_core::ProviderId;

    #[test]
    fn defaults_are_safe() {
        let config = Config::default();
        config.validate().unwrap();
        assert_eq!(config.default_provider, ProviderId::Fake);
        assert!(!config.policy.allow_send);
        assert_eq!(config.model.backend, ModelBackend::None);
    }

    #[test]
    fn parses_a_full_configuration() {
        let config = parse(
            r#"
            default_provider = "maildir"

            [policy]
            allow_send = true
            max_body_chars = 4000
            approval_ttl_hours = 24

            [maildir]
            path = "~/Mail"
            from = "Me <me@example.com>"

            [maildir.smtp]
            host = "smtp.example.com"
            port = 587
            username = "me@example.com"
            password_env = "HERALD_SMTP_PASSWORD"
            security = "starttls"

            [model]
            backend = "openai"
            endpoint = "http://127.0.0.1:11434/v1"
            model_name = "qwen2.5:1.5b-instruct"
            "#,
        )
        .unwrap();

        assert_eq!(config.default_provider, ProviderId::Maildir);
        assert!(config.policy.allow_send);
        let maildir = config.maildir.unwrap();
        assert_eq!(maildir.from.as_deref(), Some("Me <me@example.com>"));
        let smtp = maildir.smtp.unwrap();
        assert_eq!(smtp.host, "smtp.example.com");
        assert_eq!(smtp.security, SmtpSecurity::Starttls);
        assert_eq!(config.model.backend, ModelBackend::OpenAi);
    }

    #[test]
    fn unknown_keys_fail_closed() {
        let error = parse(
            r#"
            default_provider = "fake"
            auto_send = true
            "#,
        )
        .unwrap_err();
        assert!(matches!(error, ConfigError::Parse(_)));
    }

    #[test]
    fn unknown_nested_keys_fail_closed() {
        let error = parse(
            r"
            [policy]
            allow_send = false
            auto_send = true
            ",
        )
        .unwrap_err();
        assert!(matches!(error, ConfigError::Parse(_)));
    }

    #[test]
    fn planned_providers_cannot_be_the_default() {
        let error = parse("default_provider = \"gmail\"").unwrap_err();
        assert!(matches!(error, ConfigError::Invalid(_)));
    }

    #[test]
    fn maildir_default_requires_a_maildir_section() {
        let error = parse("default_provider = \"maildir\"").unwrap_err();
        assert!(matches!(error, ConfigError::Invalid(_)));
    }

    #[test]
    fn maildir_default_with_a_path_is_valid() {
        let config = parse(
            r#"
            default_provider = "maildir"
            [maildir]
            path = "/tmp/Mail"
            "#,
        )
        .unwrap();
        assert_eq!(config.default_provider, ProviderId::Maildir);
    }

    #[test]
    fn openai_backend_requires_an_endpoint_and_model() {
        let error = parse(
            r#"
            [model]
            backend = "openai"
            "#,
        )
        .unwrap_err();
        assert!(matches!(error, ConfigError::Invalid(_)));

        let error = parse(
            r#"
            [model]
            backend = "openai"
            endpoint = "http://127.0.0.1:11434/v1"
            "#,
        )
        .unwrap_err();
        assert!(matches!(error, ConfigError::Invalid(_)));
    }

    #[test]
    fn openai_backend_rejects_a_non_http_endpoint() {
        let error = parse(
            r#"
            [model]
            backend = "openai"
            endpoint = "file:///etc/passwd"
            model_name = "x"
            "#,
        )
        .unwrap_err();
        assert!(matches!(error, ConfigError::Invalid(_)));
    }

    #[test]
    fn smtp_username_requires_a_password_source() {
        let error = parse(
            r#"
            [maildir]
            path = "/tmp/Mail"
            [maildir.smtp]
            host = "smtp.example.com"
            username = "me@example.com"
            "#,
        )
        .unwrap_err();
        assert!(matches!(error, ConfigError::Invalid(_)));
    }

    #[test]
    fn environment_overrides_beat_the_file() {
        let config = parse_with_env(
            r#"
            default_provider = "fake"
            [policy]
            allow_send = false
            "#,
            |key| match key {
                "HERALD_PROVIDER" => Some("maildir".into()),
                "HERALD_MAILDIR_PATH" => Some("/tmp/Mail".into()),
                "HERALD_ALLOW_SEND" => Some("true".into()),
                _ => None,
            },
        )
        .unwrap();

        assert_eq!(config.default_provider, ProviderId::Maildir);
        assert!(config.policy.allow_send);
        assert_eq!(config.maildir.unwrap().path, PathBuf::from("/tmp/Mail"));
    }

    #[test]
    fn invalid_boolean_environment_value_is_rejected() {
        let error = parse_with_env("", |key| {
            (key == "HERALD_ALLOW_SEND").then(|| "sometimes".into())
        })
        .unwrap_err();
        assert!(matches!(error, ConfigError::Env(_)));
    }

    #[test]
    fn password_is_redacted_from_debug() {
        let config = parse(
            r#"
            [maildir]
            path = "/tmp/Mail"
            [maildir.smtp]
            host = "smtp.example.com"
            password = "hunter2"
            "#,
        )
        .unwrap();
        let rendered = format!("{config:?}");
        assert!(!rendered.contains("hunter2"));
        assert!(rendered.contains("redacted"));
    }

    #[test]
    fn password_resolution_prefers_the_direct_value() {
        let config = parse(
            r#"
            [maildir]
            path = "/tmp/Mail"
            [maildir.smtp]
            host = "smtp.example.com"
            username = "me"
            password = "hunter2"
            password_env = "HERALD_SMTP_PASSWORD"
            "#,
        )
        .unwrap();
        let smtp = config.maildir.unwrap().smtp.unwrap();
        assert_eq!(
            smtp.resolve_password(|_| None).unwrap().as_deref(),
            Some("hunter2")
        );
    }

    #[test]
    fn password_resolution_reads_the_configured_environment_variable() {
        let config = parse(
            r#"
            [maildir]
            path = "/tmp/Mail"
            [maildir.smtp]
            host = "smtp.example.com"
            username = "me"
            password_env = "MAIL_PASSWORD"
            "#,
        )
        .unwrap();
        let smtp = config.maildir.unwrap().smtp.unwrap();
        let password = smtp
            .resolve_password(|key| (key == "MAIL_PASSWORD").then(|| "secret".into()))
            .unwrap();
        assert_eq!(password.as_deref(), Some("secret"));
    }

    #[test]
    fn missing_password_environment_variable_is_reported() {
        let config = parse(
            r#"
            [maildir]
            path = "/tmp/Mail"
            [maildir.smtp]
            host = "smtp.example.com"
            username = "me"
            password_env = "MISSING_PASSWORD"
            "#,
        )
        .unwrap();
        let smtp = config.maildir.unwrap().smtp.unwrap();
        assert!(smtp.resolve_password(|_| None).is_err());
    }

    #[test]
    fn flat_keys_never_expose_a_password() {
        let config = parse(
            r#"
            [maildir]
            path = "/tmp/Mail"
            [maildir.smtp]
            host = "smtp.example.com"
            password = "hunter2"
            "#,
        )
        .unwrap();
        let flat = config.flat();
        assert!(flat.iter().all(|(_, value)| !value.contains("hunter2")));
        assert_eq!(config.get("policy.allow_send").as_deref(), Some("false"));
        assert_eq!(config.get("nope"), None);
    }

    #[test]
    fn template_is_valid_and_private() {
        let config = parse(TEMPLATE).unwrap();
        config.validate().unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        write_template(&path, false).unwrap();
        assert!(path.exists());
        assert!(write_template(&path, false).is_err());
        write_template(&path, true).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn tilde_paths_expand_without_touching_the_filesystem() {
        let path = expand_tilde(Path::new("~/Mail"), Path::new("/home/sam"));
        assert_eq!(path, PathBuf::from("/home/sam/Mail"));
        let absolute = expand_tilde(Path::new("/var/mail"), Path::new("/home/sam"));
        assert_eq!(absolute, PathBuf::from("/var/mail"));
    }
}
