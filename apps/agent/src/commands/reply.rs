//! `herald reply` — draft a reply to one thread, and queue it only when asked.
//!
//! The thread id comes from the user, never from the model: on this path the
//! model writes text and nothing else. That is the deterministic
//! draft -> approve -> send loop, and it exists because a useful draft needs
//! the conversation, which `ask` only has after target resolution.
//!
//! It still goes through the same ladder as every other command:
//! `validate_plan` -> `authorize` -> approval. It adds no authority.

use herald_core::{ActionPlan, Operation, Target, ThreadId};

use crate::app::App;
use crate::cli::EngineArg;
use crate::commands::{ask, resolve_provider};
use crate::error::AppError;
use crate::planner;

/// What an empty `--convey` means: answer the conversation itself.
const DEFAULT_CONVEY: &str =
    "(none given — answer the last message in the conversation appropriately)";

pub async fn run(
    app: &App,
    id: &str,
    convey: Option<&str>,
    provider_override: Option<&str>,
    engine: EngineArg,
    enable_send: bool,
) -> Result<u8, AppError> {
    let (provider, adapter) = resolve_provider(app, provider_override)?;
    let adapter = adapter.ok_or_else(|| {
        AppError::new(
            "no_adapter",
            format!("no {provider} adapter is implemented in this build"),
        )
    })?;

    let Some(model) =
        planner::build_model(engine, &app.config, &app.data_dir, &app.models_dir).await?
    else {
        return Err(AppError::new(
            "model_not_configured",
            "reply drafting needs a model; set [model] backend or pass --engine model",
        ));
    };

    let thread_id = ThreadId::new(id);
    let thread = adapter.get_thread(&thread_id).await?;
    if thread.messages.is_empty() {
        return Err(AppError::not_found(format!(
            "thread {id} has no messages; nothing was drafted"
        )));
    }

    let addressed_to = thread
        .messages
        .last()
        .map(|message| message.sender.clone())
        .unwrap_or_default();
    let convey = convey
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_CONVEY);

    let body = model
        .draft(
            Some(&thread),
            &addressed_to,
            convey,
            app.config.draft.tone.as_deref(),
            app.config.draft.length,
        )
        .await?;
    let body = body.trim();
    if body.is_empty() {
        return Err(AppError::new(
            "model",
            "the model returned an empty draft; nothing was queued",
        ));
    }

    // Without `--enable-send` this is a draft the user asked to see. With it,
    // the exact body becomes a pending approval — still not a send.
    let operation = if enable_send {
        Operation::SendReply
    } else {
        Operation::PrepareReply
    };
    let plan = ActionPlan {
        provider,
        operation,
        target: Target::Thread {
            thread_id: thread_id.clone(),
        },
        reply_body: Some(body.to_string()),
        needs_clarification: false,
        clarification_question: None,
    };

    ask::execute_plan(
        app,
        plan,
        Some(&model),
        enable_send,
        enable_send.then_some(body),
        "reply",
    )
    .await
}
