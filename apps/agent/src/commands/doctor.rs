//! `herald doctor` — one local, read-only diagnosis.

use std::path::{Path, PathBuf};

use serde_json::json;

use herald_config::ModelBackend;
use herald_core::ActionState;
use herald_provider::ProviderHealth;

use crate::app::App;
use crate::error::AppError;
use crate::output::format_time_ms;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    Ok,
    Warn,
    Fail,
}

impl Level {
    const fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Fail => "fail",
        }
    }
}

struct Check {
    level: Level,
    name: String,
    detail: String,
}

impl Check {
    fn ok(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            level: Level::Ok,
            name: name.into(),
            detail: detail.into(),
        }
    }

    fn warn(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            level: Level::Warn,
            name: name.into(),
            detail: detail.into(),
        }
    }

    fn fail(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            level: Level::Fail,
            name: name.into(),
            detail: detail.into(),
        }
    }
}

#[allow(clippy::too_many_lines)]
pub async fn run(app: &App) -> Result<u8, AppError> {
    let mut checks = Vec::new();

    checks.push(Check::ok(
        "version",
        format!("{} ({})", env!("CARGO_PKG_VERSION"), std::env::consts::OS),
    ));

    checks.push(Check::ok("data dir", app.data_dir.display().to_string()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(&app.data_dir) {
            Ok(metadata) => {
                let mode = metadata.permissions().mode() & 0o777;
                if mode % 0o100 == 0 {
                    checks.push(Check::ok("data perms", format!("{mode:o}")));
                } else {
                    checks.push(Check::warn(
                        "data perms",
                        format!(
                            "mode {mode:o}; run `chmod 700 {}` so other users cannot read local state",
                            app.data_dir.display()
                        ),
                    ));
                }
            }
            Err(error) => checks.push(Check::fail("data perms", error.to_string())),
        }
    }

    checks.push(if app.config_exists {
        Check::ok("config", app.config_path.display().to_string())
    } else {
        Check::warn(
            "config",
            format!(
                "no file at {}; defaults are in use (run `herald init`)",
                app.config_path.display()
            ),
        )
    });

    checks.push(if app.config.policy.allow_send {
        Check::warn(
            "send policy",
            "allow_send = true; sends still require `herald approve`",
        )
    } else {
        Check::ok("send policy", "disabled until an approval is enabled")
    });

    checks.push(Check::ok(
        "storage",
        format!(
            "{} messages, {} audit events, {} pending approval(s)",
            app.store.message_count()?,
            app.store.audit_count()?,
            app.store.approval_count()?
        ),
    ));

    let actions = app.store.recent_actions(200)?;
    let stale = actions
        .iter()
        .filter(|action| {
            action.state == ActionState::Executing
                && herald_core::time::now_ms().saturating_sub(action.created_at_ms) > 60_000
        })
        .count();
    if stale > 0 {
        checks.push(Check::warn(
            "stale actions",
            format!("{stale} action(s) stuck in executing; run `herald reconcile <id>`"),
        ));
    }

    for id in herald_core::ProviderId::ALL {
        match app.registry.get(id) {
            Some(adapter) => {
                let (level, detail) = match adapter.health().await {
                    ProviderHealth::Ready => (Level::Ok, "ready".to_string()),
                    ProviderHealth::NotAuthenticated => {
                        (Level::Warn, "not authenticated".to_string())
                    }
                    ProviderHealth::Unavailable(reason) => (Level::Fail, reason),
                };
                checks.push(Check {
                    level,
                    name: format!("provider {id}"),
                    detail,
                });
            }
            None if id.is_implemented() => {
                if app.config.default_provider == id {
                    checks.push(Check::fail(
                        format!("provider {id}"),
                        "selected as default_provider but not configured",
                    ));
                } else {
                    checks.push(Check::ok(
                        format!("provider {id}"),
                        "not configured (optional)",
                    ));
                }
            }
            None => checks.push(Check::ok(
                format!("provider {id}"),
                "planned; no adapter in this build",
            )),
        }
    }

    if let Some(maildir) = &app.config.maildir {
        if let Some((root, sent)) = app.config.maildir_paths() {
            if root.is_dir() {
                checks.push(Check::ok("maildir", root.display().to_string()));
            } else {
                checks.push(Check::fail(
                    "maildir",
                    format!("{} is not a directory", root.display()),
                ));
            }
            checks.push(Check::ok(
                "sent folder",
                format!(
                    "{} ({})",
                    sent.display(),
                    if sent.is_dir() {
                        "exists"
                    } else {
                        "created on first send"
                    }
                ),
            ));
        }

        match &maildir.smtp {
            None => checks.push(Check::warn(
                "smtp",
                "not configured; the maildir is read-only",
            )),
            Some(smtp) => {
                checks.push(Check::ok(
                    "smtp",
                    format!("{}:{} ({:?})", smtp.host, smtp.port, smtp.security),
                ));
                match smtp.resolve_password(|key| std::env::var(key).ok()) {
                    Ok(Some(_)) => checks.push(Check::ok("smtp auth", "password available")),
                    Ok(None) => checks.push(Check::warn("smtp auth", "no auth configured")),
                    Err(error) => checks.push(Check::fail("smtp auth", error.to_string())),
                }
                if smtp.stores_plaintext_password() {
                    checks.push(Check::warn(
                        "smtp secret",
                        "password is stored in config.toml; prefer password_env",
                    ));
                }
            }
        }
    }

    if let Some(imap) = &app.config.imap {
        if imap.stores_plaintext_password() {
            checks.push(Check::warn(
                "imap secret",
                "password is stored in config.toml; prefer password_env",
            ));
        }
        checks.push(Check::ok(
            "imap",
            format!("{}:{} ({})", imap.host, imap.port, imap.folder),
        ));
    }

    match app.config.model.backend {
        ModelBackend::None => checks.push(Check::ok("model", "none; rules only")),
        ModelBackend::OpenAi => {
            checks.push(Check::ok(
                "model",
                format!(
                    "openai-compatible {} ({})",
                    app.config.model.endpoint.as_deref().unwrap_or("?"),
                    app.config.model.model_name.as_deref().unwrap_or("?")
                ),
            ));
        }
        ModelBackend::OpenCode => {
            let binary = std::env::var_os("HERALD_OPENCODE_PATH")
                .map_or_else(|| PathBuf::from("opencode"), PathBuf::from);
            let model = app
                .config
                .model
                .model_name
                .as_deref()
                .unwrap_or(herald_runtime::opencode::DEFAULT_MODEL);
            match probe_version(&binary, std::time::Duration::from_secs(10)).await {
                Ok(version) => {
                    checks.push(Check::ok("model", format!("opencode {version} ({model})")));
                }
                Err(error) => checks.push(Check::fail(
                    "opencode",
                    format!("{}: {error}", binary.display()),
                )),
            }
        }
        ModelBackend::Managed => {
            let manifest_path = app
                .config
                .model
                .manifest
                .clone()
                .unwrap_or_else(|| app.models_dir.join("manifest.json"));
            match herald_runtime::ModelManifest::load(&manifest_path) {
                Ok(manifest) => {
                    let model = app.models_dir.join(&manifest.filename);
                    if model.exists() {
                        let size = std::fs::metadata(&model).map_or(0, |meta| meta.len());
                        checks.push(Check::ok(
                            "model",
                            format!("{} ({} bytes)", model.display(), size),
                        ));
                    } else {
                        checks.push(Check::fail(
                            "model",
                            format!(
                                "{} is missing; run `herald model download`",
                                model.display()
                            ),
                        ));
                    }
                }
                Err(error) => checks.push(Check::fail(
                    "model manifest",
                    format!("{}: {error}", manifest_path.display()),
                )),
            }

            match find_binary("llama-server") {
                Some(path) => checks.push(Check::ok("llama-server", path.display().to_string())),
                None => checks.push(Check::fail(
                    "llama-server",
                    "not found; set HERALD_LLAMA_SERVER or install llama.cpp",
                )),
            }
        }
    }

    let problems = checks.iter().filter(|c| c.level == Level::Fail).count();
    let warnings = checks.iter().filter(|c| c.level == Level::Warn).count();

    for check in &checks {
        let label = match check.level {
            Level::Ok => app.out.green("ok"),
            Level::Warn => app.out.amber("warn"),
            Level::Fail => app.out.red("fail"),
        };
        app.out
            .line(format!("[{label}] {:<22} {}", check.name, check.detail));
    }
    app.out.blank();
    if problems == 0 {
        app.out.line(format!(
            "{problems} problem(s), {warnings} warning(s). Herald can run."
        ));
    } else {
        app.out.line(format!(
            "{problems} problem(s), {warnings} warning(s). Fix the failures before relying on Herald."
        ));
    }

    let items = checks
        .iter()
        .map(|check| {
            json!({
                "name": check.name,
                "status": check.level.label(),
                "detail": check.detail,
            })
        })
        .collect::<Vec<_>>();

    app.out.data(
        "doctor",
        json!({
            "status": if problems == 0 { "ok" } else { "attention" },
            "problems": problems,
            "warnings": warnings,
            "checks": items,
            "generated_at": format_time_ms(herald_core::time::now_ms()),
        }),
    );

    Ok(u8::from(problems > 0))
}

async fn probe_version(
    binary: &Path,
    timeout: std::time::Duration,
) -> Result<String, std::io::Error> {
    let mut command = tokio::process::Command::new(binary);
    command
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let output = tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::TimedOut, "version check timed out")
        })??;
    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "--version exited with {}",
            output.status
        )));
    }
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(if version.is_empty() {
        "version unknown".to_string()
    } else {
        version
    })
}

fn find_binary(name: &str) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("HERALD_LLAMA_SERVER") {
        let path = PathBuf::from(path);
        return path.exists().then_some(path);
    }

    if Path::new(name).is_absolute() {
        return Path::new(name).exists().then(|| PathBuf::from(name));
    }

    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(name))
            .find(|candidate| candidate.is_file())
    })
}
