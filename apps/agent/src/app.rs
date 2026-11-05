//! Application context and command dispatch.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::CommandFactory;
use clap_complete::Shell;

use herald_config::{
    Config, Loaded, default_config_path, ensure_private_dir, load, permission_warning,
    resolve_data_dir, write_template,
};
use herald_core::{ProviderId, time::now_ms};
use herald_provider::CommunicationProvider;
use herald_storage::{AuditEvent, Store};

use crate::cli::{Cli, Command};
use crate::commands;
use crate::error::AppError;
use crate::output::Out;
use crate::registry::Registry;

pub struct App {
    pub out: Out,
    pub config: Config,
    pub config_path: PathBuf,
    pub config_exists: bool,
    pub data_dir: PathBuf,
    pub models_dir: PathBuf,
    pub store: Store,
    pub registry: Registry,
}

impl App {
    pub fn load(cli: &Cli) -> Result<Self, AppError> {
        let out = Out::new();
        let data_dir = resolve_data_dir(cli.data_dir.as_deref())?;
        ensure_private_dir(&data_dir)?;

        let loaded: Loaded = load(cli.config.as_deref())?;
        if let Some(path) = loaded.path()
            && let Some(warning) = permission_warning(path)
        {
            out.warn(warning);
        }
        if let Some(warning) = loaded.config.maildir.as_ref().and_then(|maildir| {
            maildir
                .smtp
                .as_ref()
                .filter(|smtp| smtp.stores_plaintext_password())
                .map(|_| {
                    "maildir.smtp.password is stored in plaintext; prefer password_env".to_string()
                })
        }) {
            out.warn(warning);
        }

        let models_dir = data_dir.join("models");
        ensure_private_dir(&models_dir)?;
        let db_path = data_dir.join("herald.db");
        let store = Store::open(&db_path)?;

        // Expired approvals are cancelled, not merely deleted: the action
        // state has to agree with the missing body, or a later approve would
        // fail with a confusing conflict.
        let now = now_ms();
        let expired: Vec<_> = store
            .pending_approvals()?
            .into_iter()
            .filter(|approval| approval.expires_at_ms <= now)
            .collect();
        for approval in expired {
            let cancels = matches!(
                store.get_action(&approval.action_id)?,
                Some(action) if action.state == herald_core::ActionState::AwaitingApproval
            );
            if cancels {
                store.cancel_action(&approval.action_id)?;
            } else {
                store.delete_approval(&approval.action_id)?;
            }
            store.append_audit(&AuditEvent {
                action_id: Some(approval.action_id.clone()),
                at_ms: now,
                status: "expired".into(),
                detail: "approval expired before a decision; nothing was sent".into(),
            })?;
        }

        let registry = Registry::build(&loaded.config, &data_dir);
        for warning in registry.warnings() {
            out.warn(warning);
        }

        Ok(Self {
            out,
            config_path: loaded
                .path()
                .map_or_else(default_config_path, Path::to_path_buf),
            config_exists: loaded.path().is_some(),
            config: loaded.config,
            data_dir,
            models_dir,
            store,
            registry,
        })
    }

    /// Resolve a provider id and its adapter, honoring the CLI override.
    pub fn provider(
        &self,
        override_name: Option<&str>,
    ) -> Result<(ProviderId, Option<Arc<dyn CommunicationProvider>>), AppError> {
        let provider = match override_name {
            Some(name) => name.parse::<ProviderId>().map_err(|_| {
                AppError::new("unknown_provider", format!("unknown provider: {name}"))
            })?,
            None => self.config.default_provider,
        };

        if !provider.is_implemented() {
            return Ok((provider, None));
        }

        let adapter = self.registry.get(provider).ok_or_else(|| {
            AppError::new(
                "no_adapter",
                format!(
                    "provider {provider} has no adapter in this process; check `herald doctor`"
                ),
            )
        })?;
        Ok((provider, Some(adapter)))
    }
}

/// Entry point used by `main`.
pub async fn run(cli: Cli) -> Result<u8, AppError> {
    // Commands that must work even when the existing config is invalid.
    match &cli.command {
        Command::Completions { shell } => {
            completions(*shell);
            return Ok(0);
        }
        Command::Config {
            action: crate::cli::ConfigAction::Path,
        } => {
            commands::config::print_path(&cli);
            return Ok(0);
        }
        Command::Config {
            action: crate::cli::ConfigAction::Init { force },
        } => {
            commands::config::init(&cli, *force)?;
            return Ok(0);
        }
        Command::Init { force } => {
            commands::init::run(&cli, *force)?;
            return Ok(0);
        }
        Command::Ui { port, open } => return crate::ui::serve(&cli, *port, *open),
        _ => {}
    }

    let app = App::load(&cli)?;
    commands::dispatch(&app, cli.command).await
}

fn completions(shell: Shell) {
    let mut command = Cli::command();
    let name = command.get_name().to_string();
    clap_complete::generate(shell, &mut command, name, &mut std::io::stdout());
}

/// Data paths for the pre-app `init` command.
pub fn prepare_paths(cli: &Cli) -> Result<(PathBuf, PathBuf), AppError> {
    let data_dir = resolve_data_dir(cli.data_dir.as_deref())?;
    ensure_private_dir(&data_dir)?;
    let config_path = commands::config::resolved_path(cli);
    Ok((data_dir, config_path))
}

/// Write the starter config if it is missing, or overwrite it when forced.
pub fn maybe_write_config(config_path: &Path, force: bool) -> Result<bool, AppError> {
    if config_path.exists() && !force {
        return Ok(false);
    }
    write_template(config_path, force)?;
    Ok(true)
}
