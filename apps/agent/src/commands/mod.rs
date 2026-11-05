//! One module per command group. Dispatch lives here.

pub mod approvals;
pub mod ask;
pub mod autopost;
pub mod config;
pub mod doctor;
pub mod eval;
pub mod history;
pub mod inbox;
pub mod init;
pub mod model;
pub mod providers;
pub mod x;

use std::sync::Arc;

use herald_config::quiet_hours_active;
use herald_core::{PolicyConfig, ProviderId, time::now_ms};
use herald_provider::CommunicationProvider;

use crate::app::App;
use crate::cli::{Command, ConfigAction, ModelAction, XAction};
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
            body_env,
        } => {
            ask::run(
                app,
                &instruction,
                provider.as_deref(),
                engine,
                enable_send,
                thread.as_deref(),
                body_env.as_deref(),
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
            scope,
            unread,
            limit,
            provider,
        } => inbox::inbox(app, scope.into(), unread, limit, provider.as_deref()).await,
        Command::Thread { id, provider } => inbox::thread(app, &id, provider.as_deref()).await,
        Command::Approvals => approvals::list(app),
        Command::Approve { id } => approvals::approve(app, &id).await,
        Command::Reject { id } => approvals::reject(app, &id),
        Command::Reconcile { id } => approvals::reconcile(app, &id).await,
        Command::History { limit } => history::history(app, limit),
        Command::Audit { limit } => history::audit(app, limit),
        Command::Providers => providers::run(app).await,
        Command::X { action } => match action {
            XAction::Login => x::login(app).await,
        },
        Command::Autopost { action } => autopost::run(app, action),
        Command::Config { action } => dispatch_config(app, action),
        Command::Model { action } => dispatch_model(app, action).await,
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
        Command::Init { .. } | Command::Completions { .. } | Command::Ui { .. } => {
            Err(AppError::new(
                "internal",
                "command should have been handled before app initialization",
            ))
        }
    }
}

fn dispatch_config(app: &App, action: ConfigAction) -> Result<u8, AppError> {
    match action {
        ConfigAction::Get { key } => config::get(app, &key),
        ConfigAction::List => Ok(config::list(app)),
        ConfigAction::Path => Ok(config::path(app)),
        ConfigAction::Init { force } => config::init_for_app(app, force),
    }
}

async fn dispatch_model(app: &App, action: ModelAction) -> Result<u8, AppError> {
    match action {
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

/// The kill switch written by `herald autopost off`.
pub const AUTOPOST_KILL_SWITCH: &str = "autopost.disabled";

/// Compute the policy view, including the rails only the process can measure.
///
/// Quiet hours, the daily budget, and the kill switch are measured here and
/// passed in; core stays pure. Every unknown fails closed: an unreadable
/// budget or kill-switch error disables the lane.
pub fn policy(app: &App, enable_send: bool) -> PolicyConfig {
    let mut policy = app.config.policy();
    policy.allow_send = policy.allow_send || enable_send;

    let now = now_ms();
    policy.autopost.quiet_now = app
        .config
        .policy
        .autopost_quiet_hours
        .as_deref()
        .is_some_and(|hours| quiet_hours_active(hours, minutes_of_day_utc(now)));
    policy.autopost.remaining_today = autopost_remaining(app, now);
    if app.data_dir.join(AUTOPOST_KILL_SWITCH).exists() {
        policy.autopost.enabled = false;
    }
    policy
}

/// Posts left in today's UTC budget. A storage error yields zero, not a guess.
#[must_use]
pub fn autopost_remaining(app: &App, now: i64) -> u32 {
    let limit = app.config.policy.autopost_daily_limit;
    let used = app
        .store
        .count_actions_by_operation_since("create_post", day_start_ms(now))
        .unwrap_or(limit);
    limit.saturating_sub(used)
}

/// Whole minutes since midnight UTC.
#[must_use]
pub fn minutes_of_day_utc(now_ms: i64) -> i64 {
    now_ms.div_euclid(60_000).rem_euclid(1_440)
}

/// Midnight UTC of the day containing `now_ms`.
#[must_use]
pub fn day_start_ms(now_ms: i64) -> i64 {
    now_ms.div_euclid(86_400_000) * 86_400_000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_day_helpers_are_utc_not_local() {
        // 2026-11-04T10:00:00Z
        let now = 1_793_786_400_000;
        assert_eq!(minutes_of_day_utc(now), 600);
        assert_eq!(day_start_ms(now), 1_793_750_400_000);
        assert_eq!(minutes_of_day_utc(day_start_ms(now)), 0);
        assert_eq!(day_start_ms(day_start_ms(now)), day_start_ms(now));
    }
}
