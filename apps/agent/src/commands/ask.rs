//! `herald ask` and `herald plan`.
//!
//! `ask` executes reads and drafts and turns sends into approvals. `plan` is a
//! dry run: it never writes an action or touches a provider.

use serde_json::{Value, json};

use herald_core::{
    Action, Decision, MessageSummary, PlanError, Target, Thread, ThreadId, authorize, validate_plan,
};
use herald_storage::ActionState;

use crate::app::App;
use crate::cli::EngineArg;
use crate::commands::{action_id, policy, resolve_provider};
use crate::error::AppError;
use crate::output::{Out, format_time_ms};
use crate::planner::{self, Model};
use crate::send::{self, ExecutionOutcome, operation_name};

#[allow(clippy::too_many_lines)]
pub async fn run(
    app: &App,
    instruction: &str,
    provider_override: Option<&str>,
    engine: EngineArg,
    enable_send: bool,
    thread_override: Option<&str>,
    body_env: Option<&str>,
) -> Result<u8, AppError> {
    let (provider, _adapter) = resolve_provider(app, provider_override)?;
    let model = model_for(app, engine).await?;

    let planned = planner::plan(engine, model.as_ref(), provider, instruction).await?;
    let Some(mut plan) = planned else {
        app.out.line("No plan for this instruction.");
        if engine == EngineArg::Rules {
            app.out
                .line("The model fallback is not enabled for this run.");
        }
        app.out
            .data("ask", json!({"status": "no_plan", "provider": provider}));
        return Ok(0);
    };

    if let Some(thread) = thread_override {
        plan.target = Target::Thread {
            thread_id: ThreadId::new(thread),
        };
    }
    if let Some(var) = body_env {
        // A caller that already holds the exact body (the UI) supplies it via
        // the environment, never argv. Empty or missing is an error.
        let body = std::env::var(var)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                AppError::new(
                    "invalid_plan",
                    format!("--body-env {var} is not set or is empty"),
                )
            })?;
        if plan.operation.needs_body() {
            plan.needs_clarification = false;
            plan.clarification_question = None;
        }
        plan.reply_body = Some(body);
    } else if let Some(model) = model.as_ref() {
        planner::draft_missing_body(model, &mut plan, instruction).await;
    }

    let action = match validate_plan(plan) {
        Ok(action) => action,
        Err(PlanError::ClarificationRequired(question)) => {
            return Ok(clarification(app, "ask", &question));
        }
        Err(PlanError::MissingReplyBody { .. }) => {
            return Ok(clarification(app, "ask", "What should the reply say?"));
        }
        Err(error) => {
            return Err(AppError::new("invalid_plan", error.to_string()));
        }
    };

    print_action(app, &action);

    let decision = authorize(&action, &policy(app, enable_send));

    match &decision {
        Decision::Allowed => {
            let id = action_id();
            send::record_action(app, &action, &id, None)?;
            let outcome = send::execute_allowed(app, &action, &id, model.as_ref()).await?;
            print_outcome(app, &outcome);

            app.out.blank();
            app.out.data(
                "ask",
                json!({
                    "status": "succeeded",
                    "action_id": id,
                    "provider": action.provider(),
                    "operation": operation_name(&action),
                    "target": action.target().describe(),
                    "outcome": outcome_json(&outcome),
                }),
            );
            Ok(0)
        }
        Decision::NeedsApproval(request) => {
            let Action::Send(send_action) = &action else {
                return Err(AppError::new(
                    "internal",
                    "policy requested approval for a non-send action",
                ));
            };
            let id = action_id();
            let queued = send::queue_send(app, send_action, &id).await?;

            app.out.blank();
            app.out.heading("Approval required");
            app.out.field("action", &queued.action_id);
            app.out.field("provider", queued.provider);
            app.out.field("target", &queued.target);
            app.out.field("thread", &queued.thread_id);
            app.out.field("body", &request.body);
            app.out.field("hash", &request.body_hash);
            app.out
                .field("expires", format_time_ms(queued.expires_at_ms));
            app.out.blank();
            app.out
                .line("Nothing has been sent. To send exactly this body:");
            app.out
                .line(format!("  herald approve {}", queued.action_id));
            app.out.line("To discard it:");
            app.out
                .line(format!("  herald reject {}", queued.action_id));

            app.out.data(
                "ask",
                json!({
                    "status": "awaiting_approval",
                    "action_id": queued.action_id,
                    "provider": queued.provider,
                    "operation": operation_name(&action),
                    "target": queued.target,
                    "thread_id": queued.thread_id,
                    "body": queued.body,
                    "body_hash": queued.body_hash,
                    "expires_at_ms": queued.expires_at_ms,
                }),
            );
            Ok(0)
        }
        Decision::Rejected(reason) => {
            let id = action_id();
            send::record_action(app, &action, &id, None)?;
            app.store.transition_action(&id, ActionState::Failed)?;
            send::audit(app, Some(&id), "rejected", reason.to_string())?;
            app.out.blank();
            app.out.line(format!("decision: rejected ({reason})"));
            Err(AppError::policy(reason.to_string()))
        }
    }
}

/// Dry run: plan, validate, and show the policy decision. No writes.
pub async fn dry_run(
    app: &App,
    instruction: &str,
    provider_override: Option<&str>,
    engine: EngineArg,
    enable_send: bool,
    thread_override: Option<&str>,
) -> Result<u8, AppError> {
    let (provider, _adapter) = resolve_provider(app, provider_override)?;
    let model = model_for(app, engine).await?;

    let planned = planner::plan(engine, model.as_ref(), provider, instruction).await?;
    let Some(mut plan) = planned else {
        app.out.line("No plan for this instruction.");
        app.out
            .data("plan", json!({"status": "no_plan", "provider": provider}));
        return Ok(0);
    };

    if let Some(thread) = thread_override {
        plan.target = Target::Thread {
            thread_id: ThreadId::new(thread),
        };
    }
    if let Some(model) = model.as_ref() {
        planner::draft_missing_body(model, &mut plan, instruction).await;
    }

    let action = match validate_plan(plan) {
        Ok(action) => action,
        Err(PlanError::ClarificationRequired(question)) => {
            return Ok(clarification(app, "plan", &question));
        }
        Err(PlanError::MissingReplyBody { .. }) => {
            return Ok(clarification(app, "plan", "What should the reply say?"));
        }
        Err(error) => return Err(AppError::new("invalid_plan", error.to_string())),
    };

    print_action(app, &action);
    let decision = authorize(&action, &policy(app, enable_send));

    let (status, detail) = match &decision {
        Decision::Allowed => ("allowed", "would execute locally".to_string()),
        Decision::NeedsApproval(request) => (
            "needs_approval",
            format!(
                "would queue an approval for {} ({} bytes, hash {})",
                request.target.describe(),
                request.body.len(),
                request.body_hash
            ),
        ),
        Decision::Rejected(reason) => ("rejected", reason.to_string()),
    };

    app.out.field("decision", status);
    app.out.field("detail", &detail);
    app.out.blank();
    app.out
        .line("Dry run: no action was recorded and no provider was changed.");

    app.out.data(
        "plan",
        json!({
            "status": status,
            "provider": action.provider(),
            "operation": operation_name(&action),
            "target": action.target().describe(),
            "decision": status,
            "detail": detail,
            "would_send": matches!(decision, Decision::NeedsApproval(_)),
        }),
    );
    Ok(0)
}

async fn model_for(app: &App, engine: EngineArg) -> Result<Option<Model>, AppError> {
    match planner::build_model(engine, &app.config, &app.data_dir, &app.models_dir).await {
        Ok(model) => Ok(model),
        Err(error) if engine == EngineArg::Auto => {
            app.out
                .warn(format!("model unavailable; using rules only ({error})"));
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn clarification(app: &App, command: &str, question: &str) -> u8 {
    app.out.field("decision", "clarification required");
    app.out.field("question", question);
    app.out.data(
        command,
        json!({
            "status": "clarification_required",
            "question": question,
        }),
    );
    0
}

fn print_action(app: &App, action: &Action) {
    app.out.field("provider", action.provider());
    app.out.field("operation", operation_name(action));
    app.out.field("target", action.target().describe());
    match action {
        Action::Draft(draft) => {
            app.out.field("body", &draft.body);
        }
        Action::Send(send) => {
            app.out.field("body", &send.body);
            app.out.field("hash", &send.body_hash);
        }
        Action::Read(_) => {}
    }
}

pub fn print_outcome(app: &App, outcome: &ExecutionOutcome) {
    match outcome {
        ExecutionOutcome::Messages {
            summaries,
            new_count,
        } => {
            app.out.blank();
            print_messages(app.out, summaries);
            app.out.field("new", format!("{new_count} recorded"));
        }
        ExecutionOutcome::Thread {
            thread,
            new_count,
            summary,
        } => {
            app.out.field("subject", &thread.subject);
            print_thread(app.out, thread);
            if let Some(summary) = summary {
                app.out.blank();
                app.out.heading("Summary");
                app.out.line(summary);
            }
            app.out.field("new", format!("{new_count} recorded"));
        }
        ExecutionOutcome::Draft { body_hash } => {
            app.out.line("draft:     prepared locally (nothing sent)");
            app.out.field("hash", body_hash);
        }
        ExecutionOutcome::Deferred { provider } => {
            app.out.line(format!(
                "execution: skipped — no {provider} adapter in this build; plan only"
            ));
        }
    }
}

pub fn print_messages(out: Out, summaries: &[MessageSummary]) {
    if summaries.is_empty() {
        out.line("messages:  none");
        return;
    }
    for summary in summaries {
        let flag = if summary.unread {
            out.amber("*")
        } else {
            " ".into()
        };
        out.line(format!(
            "{flag} {:<32} {:<40} [{}]",
            summary.sender, summary.subject, summary.thread_id
        ));
        out.line(format!("  id: {}", summary.id));
    }
}

pub fn print_thread(out: Out, thread: &Thread) {
    for message in &thread.messages {
        out.line(format!(
            "\n--- {}  {} ---",
            message.sender,
            format_time_ms(message.timestamp_ms)
        ));
        let body: String = message.body.as_ref().chars().take(2_000).collect();
        out.line(body);
    }
}

pub fn messages_json(summaries: &[MessageSummary]) -> Value {
    Value::Array(
        summaries
            .iter()
            .map(|summary| {
                json!({
                    "id": summary.id,
                    "thread_id": summary.thread_id,
                    "sender": summary.sender,
                    "subject": summary.subject,
                    "timestamp_ms": summary.timestamp_ms,
                    "unread": summary.unread,
                })
            })
            .collect(),
    )
}

pub fn thread_json(thread: &Thread) -> Value {
    json!({
        "id": thread.id,
        "provider": thread.provider,
        "subject": thread.subject,
        "messages": thread.messages.iter().map(|message| json!({
            "id": message.id,
            "thread_id": message.thread_id,
            "sender": message.sender,
            "subject": message.subject,
            "body": message.body.as_ref(),
            "timestamp_ms": message.timestamp_ms,
            "unread": message.unread,
        })).collect::<Vec<_>>(),
    })
}

fn outcome_json(outcome: &ExecutionOutcome) -> Value {
    match outcome {
        ExecutionOutcome::Messages {
            summaries,
            new_count,
        } => json!({
            "kind": "messages",
            "messages": messages_json(summaries),
            "new_count": new_count,
        }),
        ExecutionOutcome::Thread {
            thread,
            new_count,
            summary,
        } => json!({
            "kind": "thread",
            "thread": thread_json(thread),
            "new_count": new_count,
            "summary": summary,
        }),
        ExecutionOutcome::Draft { body_hash } => {
            json!({"kind": "draft", "body_hash": body_hash, "sent": false})
        }
        ExecutionOutcome::Deferred { provider } => {
            json!({"kind": "deferred", "provider": provider})
        }
    }
}
