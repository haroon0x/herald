//! One module per command group. Dispatch lives here.

pub mod approvals;
pub mod ask;
pub mod config;
pub mod doctor;
pub mod eval;
pub mod history;
pub mod inbox;
pub mod init;
pub mod model;
pub mod providers;

use std::sync::Arc;

use herald_core::{PolicyConfig, ProviderId};
use herald_provider::CommunicationProvider;

use crate::app::App;
use crate::cli::{Command, ConfigAction, ModelAction};
use crate::error::AppError;

pub async fn dispatch(app: &App, command: Command) -> Result<u8, AppError> {
    match command {
        Command::Doctor => doctor::run(app).await,
        Command::Ask {
            instruction,
            provider,
            engine,
            enable_send,
            thread,
        } => {
            ask::run(
                app,
                &instruction,
                provider.as_deref(),
                engine,
                enable_send,
                thread.as_deref(),
            )
            .await
        }
        Command::Plan {
            instruction,
            provider,
            engine,
            enable_send,
            thread,
        } => {
            ask::dry_run(
                app,
                &instruction,
                provider.as_deref(),
                engine,
                enable_send,
                thread.as_deref(),
            )
            .await
        }
        Command::Inbox {
            unread,
            limit,
            provider,
        } => inbox::inbox(app, unread, limit, provider.as_deref()).await,
        Command::Thread { id, provider } => inbox::thread(app, &id, provider.as_deref()).await,
        Command::Approvals => approvals::list(app),
        Command::Approve { id } => approvals::approve(app, &id).await,
        Command::Reject { id } => approvals::reject(app, &id),
        Command::Reconcile { id } => approvals::reconcile(app, &id).await,
        Command::History { limit } => history::history(app, limit),
        Command::Audit { limit } => history::audit(app, limit),
        Command::Providers => providers::run(app).await,
        Command::Config { action } => match action {
            ConfigAction::Get { key } => config::get(app, &key),
            ConfigAction::List => Ok(config::list(app)),
            ConfigAction::Path => Ok(config::path(app)),
            ConfigAction::Init { force } => config::init_for_app(app, force),
        },
        Command::Model { action } => match action {
            ModelAction::Download => model::download(app).await,
            ModelAction::Verify => model::verify(app),
            ModelAction::Status => Ok(model::status(app)),
            ModelAction::Pin {
                file,
                name,
                repo,
                revision,
                quantization,
                manifest,
            } => model::pin(
                app,
                &file,
                &name,
                &repo,
                &revision,
                &quantization,
                manifest.as_deref(),
            ),
        },
        Command::Eval {
            engine,
            eval_file,
            min_score,
            provider,
        } => {
            eval::run(
                app,
                engine,
                eval_file.as_deref(),
                min_score,
                provider.as_deref(),
            )
            .await
        }
        // Handled before the app is loaded.
        Command::Init { .. } | Command::Completions { .. } => Err(AppError::new(
            "internal",
            "command should have been handled before app initialization",
        )),
    }
}

pub fn resolve_provider(
    app: &App,
    override_name: Option<&str>,
) -> Result<(ProviderId, Option<Arc<dyn CommunicationProvider>>), AppError> {
    app.provider(override_name)
}

pub fn action_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub fn policy(app: &App, enable_send: bool) -> PolicyConfig {
    let mut policy = app.config.policy();
    policy.allow_send = policy.allow_send || enable_send;
    policy
}
