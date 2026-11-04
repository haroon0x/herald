//! Execution of validated actions, the approval queue, and reconciliation.
//!
//! The rules of this module:
//! - reads and drafts are idempotent and local;
//! - a send is created as an approval, never executed by `ask`;
//! - `approve` re-resolves the target and re-checks the body hash;
//! - a timeout becomes `uncertain`, which is reconciled, never retried.

use herald_core::{
    Action, ActionState, MessageQuery, MessageSummary, ProviderId, ReadAction, ReadOperation,
    SendAction, Target, Thread, ThreadId, body_hash, time::now_ms,
};
use herald_provider::{CommunicationProvider, ExecutionReceipt, ProviderError};
use herald_storage::{AuditEvent, PendingApproval, StoredMessage, new_action};

use crate::app::App;
use crate::error::AppError;
use crate::planner::Model;

const LIST_LIMIT: usize = 20;
const RESOLVE_LIMIT: usize = 50;
const STALE_EXECUTING_MS: i64 = 60_000;

#[derive(Debug)]
pub enum ExecutionOutcome {
    Messages {
        summaries: Vec<MessageSummary>,
        new_count: usize,
    },
    Thread {
        thread: Thread,
        new_count: usize,
        summary: Option<String>,
    },
    Draft {
        body_hash: String,
    },
    /// A planned read for a provider that has no adapter in this build.
    Deferred {
        provider: ProviderId,
    },
}

impl ExecutionOutcome {
    #[must_use]
    pub fn audit_detail(&self) -> String {
        match self {
            Self::Messages {
                summaries,
                new_count,
            } => format!("{} message(s), {new_count} new", summaries.len()),
            Self::Thread {
                thread, new_count, ..
            } => format!("{} message(s), {new_count} new", thread.messages.len()),
            Self::Draft { body_hash } => format!("draft {body_hash} prepared locally"),
            Self::Deferred { provider } => format!("no {provider} adapter; plan deferred"),
        }
    }
}

/// A send that is waiting for a human decision.
#[derive(Debug)]
pub struct QueuedApproval {
    pub action_id: String,
    pub provider: ProviderId,
    pub target: String,
    pub thread_id: ThreadId,
    pub body: String,
    pub body_hash: String,
    pub expires_at_ms: i64,
}

#[derive(Debug)]
pub struct SendOutcome {
    pub status: &'static str,
    pub message_id: Option<String>,
    pub verified: bool,
    pub detail: String,
}

/// Execute an allowed read or draft action and drive its state machine.
pub async fn execute_allowed(
    app: &App,
    action: &Action,
    action_id: &str,
    model: Option<&Model>,
) -> Result<ExecutionOutcome, AppError> {
    if let Action::Send(_) = action {
        return Err(AppError::new(
            "invalid_state",
            "sends must pass through the approval queue",
        ));
    }

    let adapter = app.registry.get(action.provider());

    if matches!(action, Action::Read(_)) && adapter.is_none() {
        audit(
            app,
            Some(action_id),
            "deferred",
            format!("no {} adapter in this build", action.provider()),
        )?;
        return Ok(ExecutionOutcome::Deferred {
            provider: action.provider(),
        });
    }

    app.store
        .transition_action(action_id, ActionState::Executing)?;
    audit(
        app,
        Some(action_id),
        "executing",
        format!("{} {}", action.provider(), action.target().describe()),
    )?;

    let result = match action {
        Action::Read(read) => execute_read(app, read, adapter.as_deref(), model).await,
        Action::Draft(draft) => Ok(ExecutionOutcome::Draft {
            body_hash: body_hash(&draft.body),
        }),
        Action::Send(_) => unreachable!("guarded above"),
    };

    match result {
        Ok(outcome) => {
            app.store
                .transition_action(action_id, ActionState::Succeeded)?;
            audit(app, Some(action_id), "succeeded", outcome.audit_detail())?;
            Ok(outcome)
        }
        Err(error) => {
            app.store
                .transition_action(action_id, ActionState::Failed)?;
            audit(app, Some(action_id), "failed", error.message.clone())?;
            Err(error)
        }
    }
}

async fn execute_read(
    app: &App,
    read: &ReadAction,
    provider: Option<&dyn CommunicationProvider>,
    model: Option<&Model>,
) -> Result<ExecutionOutcome, AppError> {
    let provider = provider.ok_or_else(|| {
        AppError::new(
            "no_adapter",
            format!("no {} adapter is implemented in this build", read.provider),
        )
    })?;

    match read.operation {
        ReadOperation::ListRecentMessages => {
            let summaries = provider
                .list_recent(&MessageQuery::recent(LIST_LIMIT))
                .await?;
            Ok(ExecutionOutcome::Messages {
                new_count: record_summaries(app, read.provider, &summaries)?,
                summaries,
            })
        }
        ReadOperation::ListUnreadMessages => {
            let summaries = provider
                .list_recent(&MessageQuery::unread(LIST_LIMIT))
                .await?;
            Ok(ExecutionOutcome::Messages {
                new_count: record_summaries(app, read.provider, &summaries)?,
                summaries,
            })
        }
        ReadOperation::SearchMessages => {
            let query = MessageQuery {
                search: Some(search_terms(&read.target)),
                limit: LIST_LIMIT,
                ..MessageQuery::default()
            };
            let summaries = provider.list_recent(&query).await?;
            Ok(ExecutionOutcome::Messages {
                new_count: record_summaries(app, read.provider, &summaries)?,
                summaries,
            })
        }
        ReadOperation::GetThread | ReadOperation::SummarizeThread => {
            let thread_id = resolve_thread(provider, &read.target).await?;
            let thread = provider.get_thread(&thread_id).await?;
            let new_count = record_thread(app, read.provider, &thread)?;
            let summary = if read.operation == ReadOperation::SummarizeThread {
                match model {
                    Some(model) => match model.summarize(&thread, "summarize this thread").await {
                        Ok(summary) if !summary.is_empty() => Some(summary),
                        Ok(_) => None,
                        Err(error) => {
                            tracing::warn!(%error, "summarization failed");
                            None
                        }
                    },
                    None => None,
                }
            } else {
                None
            };
            Ok(ExecutionOutcome::Thread {
                thread,
                new_count,
                summary,
            })
        }
    }
}

/// Resolve a planner target against real provider data.
///
/// A description is not an identifier. Resolution fails closed when nothing
/// matches or when a description matches more than one thread.
pub async fn resolve_thread(
    provider: &dyn CommunicationProvider,
    target: &Target,
) -> Result<ThreadId, AppError> {
    match target {
        Target::Thread { thread_id } => {
            provider.get_thread(thread_id).await?;
            Ok(thread_id.clone())
        }
        Target::LatestFrom { sender } => {
            let summaries = provider
                .list_recent(&MessageQuery::recent(RESOLVE_LIMIT))
                .await?;
            latest_matching(&summaries, sender)
                .map(|summary| summary.thread_id.clone())
                .ok_or_else(|| {
                    AppError::new(
                        "unresolved_target",
                        format!("no message from {sender:?} was found"),
                    )
                })
        }
        Target::Timeline => Ok(ThreadId::new("timeline")),
        Target::Description { description } => {
            let query = MessageQuery {
                search: Some(search_terms(target)),
                limit: RESOLVE_LIMIT,
                ..MessageQuery::default()
            };
            let summaries = provider.list_recent(&query).await?;
            let mut threads: Vec<ThreadId> = summaries
                .iter()
                .map(|summary| summary.thread_id.clone())
                .collect();
            threads.sort_by(|left, right| left.as_str().cmp(right.as_str()));
            threads.dedup();

            match threads.as_slice() {
                [] => Err(AppError::new(
                    "unresolved_target",
                    format!("no thread matches {description:?}"),
                )),
                [only] => Ok(only.clone()),
                many => Err(AppError::new(
                    "ambiguous_target",
                    format!(
                        "{} threads match {description:?}; narrow the target",
                        many.len()
                    ),
                )),
            }
        }
    }
}

fn latest_matching<'a>(
    summaries: &'a [MessageSummary],
    sender: &str,
) -> Option<&'a MessageSummary> {
    let needle = sender.to_lowercase();
    let needle = needle.trim_start_matches("the ").trim();
    summaries
        .iter()
        .find(|summary| summary.sender.to_lowercase().contains(needle))
}

fn search_terms(target: &Target) -> String {
    match target {
        Target::Description { description } => strip_search_prefix(description),
        Target::LatestFrom { sender } => sender.clone(),
        Target::Thread { thread_id } => thread_id.as_str().to_string(),
        Target::Timeline => "timeline".to_string(),
    }
}

fn strip_search_prefix(description: &str) -> String {
    const PREFIXES: [&str; 7] = [
        "search for messages about ",
        "search for messages ",
        "search for ",
        "search ",
        "find the ",
        "find ",
        "messages about ",
    ];

    let mut text = description.trim().trim_end_matches('.').trim();
    for prefix in PREFIXES {
        if matches_prefix(text, prefix) {
            text = &text[prefix.len()..];
            break;
        }
    }
    let text = text.trim();
    if matches_prefix(text, "the ") {
        return text[4..].trim().to_string();
    }
    text.to_string()
}

fn matches_prefix(text: &str, prefix: &str) -> bool {
    text.len() >= prefix.len()
        && text
            .get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

fn record_summaries(
    app: &App,
    provider: ProviderId,
    summaries: &[MessageSummary],
) -> Result<usize, AppError> {
    let mut new_count = 0;
    for summary in summaries {
        if app
            .store
            .record_message(&StoredMessage::from_summary(provider, summary))?
        {
            new_count += 1;
        }
    }
    Ok(new_count)
}

fn record_thread(app: &App, provider: ProviderId, thread: &Thread) -> Result<usize, AppError> {
    let mut new_count = 0;
    for message in &thread.messages {
        let stored = StoredMessage {
            provider,
            message_id: message.id.clone(),
            thread_id: message.thread_id.clone(),
            sender: message.sender.clone(),
            subject: message.subject.clone(),
            timestamp_ms: message.timestamp_ms,
            unread: message.unread,
        };
        if app.store.record_message(&stored)? {
            new_count += 1;
        }
    }
    Ok(new_count)
}

/// Create the persistent action record for a validated action.
pub fn record_action(
    app: &App,
    action: &Action,
    action_id: &str,
    thread_id: Option<&ThreadId>,
) -> Result<(), AppError> {
    let target_json = serde_json::to_string(action.target())
        .map_err(|error| AppError::new("internal", format!("serializing target: {error}")))?;
    let content_hash = match action {
        Action::Send(send) => Some(send.body_hash.clone()),
        Action::Draft(draft) => Some(body_hash(&draft.body)),
        Action::Read(_) => None,
    };

    let mut record = new_action(
        action_id,
        action.provider(),
        operation_name(action),
        action.target().describe(),
        content_hash,
    );
    record.target_json = Some(target_json);
    record.thread_id = thread_id.cloned();
    app.store.create_action(&record)?;
    Ok(())
}

#[must_use]
pub fn operation_name(action: &Action) -> &'static str {
    match action {
        Action::Read(read) => match read.operation {
            ReadOperation::ListRecentMessages => "list_recent_messages",
            ReadOperation::ListUnreadMessages => "list_unread_messages",
            ReadOperation::GetThread => "get_thread",
            ReadOperation::SearchMessages => "search_messages",
            ReadOperation::SummarizeThread => "summarize_thread",
        },
        Action::Draft(_) => "prepare_reply",
        Action::Send(send) => match send.target {
            Target::Timeline => "create_post",
            _ => "send_reply",
        },
    }
}

/// Turn a send into a pending approval. Never sends.
pub async fn queue_send(
    app: &App,
    action: &SendAction,
    action_id: &str,
) -> Result<QueuedApproval, AppError> {
    let provider = app.registry.get(action.provider).ok_or_else(|| {
        AppError::new(
            "no_adapter",
            format!(
                "no {} adapter is implemented in this build; nothing was queued",
                action.provider
            ),
        )
    })?;

    let resolved = match resolve_thread(provider.as_ref(), &action.target).await {
        Ok(thread) => thread,
        Err(error) => {
            record_action(app, &Action::Send(action.clone()), action_id, None)?;
            app.store
                .transition_action(action_id, ActionState::Failed)?;
            audit(app, Some(action_id), "failed", error.message.clone())?;
            return Err(error);
        }
    };

    record_action(
        app,
        &Action::Send(action.clone()),
        action_id,
        Some(&resolved),
    )?;

    let ttl_ms = i64::try_from(app.config.policy.approval_ttl_hours)
        .unwrap_or(72)
        .saturating_mul(3_600_000);
    let now = now_ms();
    let expires_at_ms = now.saturating_add(ttl_ms);

    let approval = PendingApproval {
        action_id: action_id.to_string(),
        provider: action.provider,
        target: action.target.clone(),
        thread_id: resolved.clone(),
        body: action.body.clone(),
        body_hash: action.body_hash.clone(),
        created_at_ms: now,
        expires_at_ms,
    };

    app.store
        .transition_action(action_id, ActionState::AwaitingApproval)?;
    if let Err(error) = app.store.create_approval(&approval) {
        // A pending action with no inspectable body must not be approvable.
        let _ = app.store.transition_action(action_id, ActionState::Failed);
        audit(
            app,
            Some(action_id),
            "failed",
            format!("could not store the approval: {error}"),
        )?;
        return Err(error.into());
    }
    audit(
        app,
        Some(action_id),
        "awaiting_approval",
        "send requires explicit confirmation".into(),
    )?;

    Ok(QueuedApproval {
        action_id: action_id.to_string(),
        provider: action.provider,
        target: action.target.describe(),
        thread_id: resolved,
        body: action.body.clone(),
        body_hash: action.body_hash.clone(),
        expires_at_ms,
    })
}

/// Approve and execute one pending send.
#[allow(clippy::too_many_lines)]
pub async fn approve(app: &App, action_id: &str) -> Result<SendOutcome, AppError> {
    let action = app
        .store
        .get_action(action_id)?
        .ok_or_else(|| AppError::not_found(format!("action {action_id} was not found")))?;

    if action.state != ActionState::AwaitingApproval {
        return Err(AppError::new(
            "invalid_state",
            format!(
                "action {action_id} is {:?}, not awaiting approval",
                action.state
            ),
        ));
    }

    let Some(approval) = app.store.get_approval(action_id)? else {
        let _ = app.store.cancel_action(action_id);
        audit(
            app,
            Some(action_id),
            "cancelled",
            "approval body was missing; action cancelled, nothing sent".into(),
        )?;
        return Err(AppError::conflict(
            "the approval body is no longer available; the action was cancelled",
        ));
    };

    let now = now_ms();
    if now >= approval.expires_at_ms {
        app.store.cancel_action(action_id)?;
        audit(
            app,
            Some(action_id),
            "expired",
            "approval expired before it was decided; nothing was sent".into(),
        )?;
        return Err(AppError::conflict("the approval expired; nothing was sent"));
    }

    let provider = app.registry.get(action.provider).ok_or_else(|| {
        AppError::new(
            "no_adapter",
            format!(
                "no {} adapter is implemented in this build",
                action.provider
            ),
        )
    })?;

    // Re-resolve immediately before sending. A changed answer is a stop
    // condition, not something to paper over. A resolution error also fails
    // closed: keeping a pending body for a target that no longer resolves is
    // worse than making the user plan again.
    let resolved = match resolve_thread(provider.as_ref(), &approval.target).await {
        Ok(resolved) => resolved,
        Err(error) => {
            let _ = app.store.transition_action(action_id, ActionState::Failed);
            let _ = app.store.delete_approval(action_id);
            audit(
                app,
                Some(action_id),
                "unresolved_at_approve",
                error.message.clone(),
            )?;
            return Err(error);
        }
    };
    if resolved != approval.thread_id || action.thread_id.as_ref().is_some_and(|id| id != &resolved)
    {
        app.store
            .transition_action(action_id, ActionState::Failed)?;
        app.store.delete_approval(action_id)?;
        audit(
            app,
            Some(action_id),
            "target_changed",
            format!(
                "target re-resolved to {} instead of {}; nothing was sent",
                resolved, approval.thread_id
            ),
        )?;
        return Err(AppError::conflict(
            "the target changed between planning and approval; nothing was sent",
        ));
    }

    if body_hash(&approval.body) != approval.body_hash
        || action.content_hash.as_deref() != Some(approval.body_hash.as_str())
    {
        app.store
            .transition_action(action_id, ActionState::Failed)?;
        app.store.delete_approval(action_id)?;
        audit(
            app,
            Some(action_id),
            "body_changed",
            "body hash mismatch; nothing was sent".into(),
        )?;
        return Err(AppError::conflict(
            "the approved body no longer matches; nothing was sent",
        ));
    }

    app.store
        .transition_action(action_id, ActionState::Executing)?;
    audit(
        app,
        Some(action_id),
        "executing",
        format!("sending to thread {resolved}"),
    )?;

    prepare_and_send(
        app,
        provider.as_ref(),
        action_id,
        &resolved,
        &approval.body,
        &approval.body_hash,
        "",
    )
    .await
}

/// Execute a post on the autopost lane.
///
/// This is reachable only from `Decision::Allowed`, which core grants only when
/// every autopost rail passed. It reuses the same prepare/send/verify path and
/// state machine as an approved send; there is no shortcut around verification.
pub async fn execute_autopost_send(
    app: &App,
    action: &SendAction,
    action_id: &str,
) -> Result<SendOutcome, AppError> {
    if !matches!(action.target, Target::Timeline) {
        return Err(AppError::new(
            "internal",
            "the autopost lane only exists for timeline posts",
        ));
    }

    let provider = app.registry.get(action.provider).ok_or_else(|| {
        AppError::new(
            "no_adapter",
            format!(
                "no {} adapter is implemented in this build",
                action.provider
            ),
        )
    })?;

    let thread = ThreadId::new("timeline");
    record_action(app, &Action::Send(action.clone()), action_id, Some(&thread))?;
    app.store
        .transition_action(action_id, ActionState::Executing)?;
    audit(
        app,
        Some(action_id),
        "autopost",
        format!("executing autonomous post {}", action.body_hash),
    )?;

    prepare_and_send(
        app,
        provider.as_ref(),
        action_id,
        &thread,
        &action.body,
        &action.body_hash,
        "autopost",
    )
    .await
}

/// The one path from a prepared action to a verified receipt.
///
/// `lane` is empty for an approved send and `autopost` for the autonomous lane;
/// it only prefixes audit details, never changes behaviour.
async fn prepare_and_send(
    app: &App,
    provider: &dyn CommunicationProvider,
    action_id: &str,
    thread: &ThreadId,
    body: &str,
    expected_hash: &str,
    lane: &str,
) -> Result<SendOutcome, AppError> {
    let prefix = if lane.is_empty() {
        String::new()
    } else {
        format!("{lane}: ")
    };

    let prepared = match provider.prepare_reply(thread, body, action_id).await {
        Ok(prepared) => prepared,
        Err(error) => {
            app.store
                .transition_action(action_id, ActionState::Failed)?;
            app.store.delete_approval(action_id)?;
            audit(app, Some(action_id), "failed", format!("{prefix}{error}"))?;
            return Err(error.into());
        }
    };

    // A provider must not rewrite the body between approval and the wire.
    if prepared.body_hash != expected_hash || body_hash(&prepared.body) != expected_hash {
        app.store
            .transition_action(action_id, ActionState::Failed)?;
        app.store.delete_approval(action_id)?;
        audit(
            app,
            Some(action_id),
            "body_changed",
            format!("{prefix}provider changed the body; nothing was sent"),
        )?;
        return Err(AppError::conflict(
            "the provider changed the body; nothing was sent",
        ));
    }

    match provider.send_reply(&prepared).await {
        Ok(receipt) => finish_send(app, action_id, &receipt, &prefix),
        Err(ProviderError::Timeout) => {
            app.store
                .transition_action(action_id, ActionState::Uncertain)?;
            app.store.delete_approval(action_id)?;
            audit(
                app,
                Some(action_id),
                "uncertain",
                format!("{prefix}delivery timed out; reconcile before doing anything else"),
            )?;
            Ok(SendOutcome {
                status: "uncertain",
                message_id: None,
                verified: false,
                detail: "delivery timed out; run `herald reconcile` to resolve it".into(),
            })
        }
        Err(error) => {
            app.store
                .transition_action(action_id, ActionState::Failed)?;
            app.store.delete_approval(action_id)?;
            audit(app, Some(action_id), "failed", format!("{prefix}{error}"))?;
            Err(error.into())
        }
    }
}

fn finish_send(
    app: &App,
    action_id: &str,
    receipt: &ExecutionReceipt,
    prefix: &str,
) -> Result<SendOutcome, AppError> {
    if receipt.verified {
        app.store
            .transition_action(action_id, ActionState::Succeeded)?;
        app.store.delete_approval(action_id)?;
        audit(
            app,
            Some(action_id),
            "succeeded",
            format!(
                "{prefix}sent message {} to thread {}",
                receipt.provider_message_id, receipt.thread_id
            ),
        )?;
        Ok(SendOutcome {
            status: "sent",
            message_id: Some(receipt.provider_message_id.as_str().to_string()),
            verified: true,
            detail: format!("sent and verified as {}", receipt.provider_message_id),
        })
    } else {
        app.store
            .transition_action(action_id, ActionState::Uncertain)?;
        app.store.delete_approval(action_id)?;
        audit(
            app,
            Some(action_id),
            "uncertain",
            format!("{prefix}delivered but not confirmed locally; reconcile before retrying"),
        )?;
        Ok(SendOutcome {
            status: "uncertain",
            message_id: Some(receipt.provider_message_id.as_str().to_string()),
            verified: false,
            detail: "delivered but not confirmed; run `herald reconcile`".into(),
        })
    }
}

/// Reject a pending action. Nothing is sent.
pub fn reject(app: &App, action_id: &str) -> Result<(), AppError> {
    let action = app
        .store
        .get_action(action_id)?
        .ok_or_else(|| AppError::not_found(format!("action {action_id} was not found")))?;

    if action.state != ActionState::AwaitingApproval {
        return Err(AppError::new(
            "invalid_state",
            format!(
                "action {action_id} is {:?}, not awaiting approval",
                action.state
            ),
        ));
    }

    app.store.cancel_action(action_id)?;
    audit(
        app,
        Some(action_id),
        "cancelled",
        "rejected by the user; nothing was sent".into(),
    )?;
    Ok(())
}

/// Resolve an `uncertain` action by asking the provider whether the message is
/// there. Never retries the send.
#[allow(clippy::too_many_lines)]
pub async fn reconcile(app: &App, action_id: &str) -> Result<SendOutcome, AppError> {
    let action = app
        .store
        .get_action(action_id)?
        .ok_or_else(|| AppError::not_found(format!("action {action_id} was not found")))?;

    if action.state == ActionState::Executing {
        let age = now_ms().saturating_sub(action.created_at_ms);
        if age < STALE_EXECUTING_MS {
            return Err(AppError::new(
                "invalid_state",
                "the action is still executing; wait before reconciling",
            ));
        }
        app.store
            .transition_action(action_id, ActionState::Uncertain)?;
        audit(
            app,
            Some(action_id),
            "recovered",
            "stale executing action moved to uncertain; no retry".into(),
        )?;
    }

    let action = app.store.get_action(action_id)?.ok_or_else(|| {
        AppError::not_found(format!("action {action_id} disappeared during reconcile"))
    })?;

    if action.state != ActionState::Uncertain {
        return Err(AppError::new(
            "invalid_state",
            format!(
                "action {action_id} is {:?}; only uncertain actions are reconciled",
                action.state
            ),
        ));
    }

    let provider = app.registry.get(action.provider).ok_or_else(|| {
        AppError::new(
            "no_adapter",
            format!(
                "no {} adapter is implemented in this build",
                action.provider
            ),
        )
    })?;

    let thread_id = match action.thread_id.clone() {
        Some(thread) => thread,
        None => app
            .store
            .get_approval(action_id)?
            .map(|approval| approval.thread_id)
            .ok_or_else(|| {
                AppError::new(
                    "no_thread",
                    "the action has no recorded thread; it cannot be reconciled",
                )
            })?,
    };

    match provider.find_sent(&thread_id, action_id).await {
        Ok(Some(receipt)) if receipt.verified => {
            app.store
                .transition_action(action_id, ActionState::Succeeded)?;
            app.store.delete_approval(action_id)?;
            audit(
                app,
                Some(action_id),
                "reconciled",
                format!(
                    "found delivered message {} in thread {}",
                    receipt.provider_message_id, thread_id
                ),
            )?;
            Ok(SendOutcome {
                status: "sent",
                message_id: Some(receipt.provider_message_id.as_str().to_string()),
                verified: true,
                detail: "the provider confirmed the message; marked succeeded".into(),
            })
        }
        Ok(Some(_)) => {
            audit(
                app,
                Some(action_id),
                "uncertain",
                "message found but not verified; still uncertain".into(),
            )?;
            Ok(SendOutcome {
                status: "uncertain",
                message_id: None,
                verified: false,
                detail: "the provider found a candidate but could not verify it".into(),
            })
        }
        Ok(None) => {
            app.store
                .transition_action(action_id, ActionState::Failed)?;
            app.store.delete_approval(action_id)?;
            audit(
                app,
                Some(action_id),
                "failed",
                "reconciliation found no delivered message; marked failed, not retried".into(),
            )?;
            Ok(SendOutcome {
                status: "failed",
                message_id: None,
                verified: false,
                detail: "no delivered message was found; nothing was retried".into(),
            })
        }
        Err(error) => {
            audit(
                app,
                Some(action_id),
                "uncertain",
                format!("reconciliation failed: {error}"),
            )?;
            Err(error.into())
        }
    }
}

pub fn audit(
    app: &App,
    action_id: Option<&str>,
    status: &str,
    detail: String,
) -> Result<(), AppError> {
    app.store.append_audit(&AuditEvent {
        action_id: action_id.map(ToString::to_string),
        at_ms: now_ms(),
        status: status.to_string(),
        detail,
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_boilerplate_is_trimmed() {
        assert_eq!(
            strip_search_prefix("Search for messages about the interview."),
            "interview"
        );
        assert_eq!(
            strip_search_prefix("Look for the lease."),
            "Look for the lease"
        );
    }

    #[test]
    fn sender_matching_strips_a_leading_the() {
        let summaries = vec![MessageSummary {
            id: herald_core::MessageId::new("m1"),
            thread_id: ThreadId::new("t1"),
            sender: "Alex Founder <alex@example.com>".into(),
            subject: "Interview".into(),
            timestamp_ms: 1,
            unread: true,
        }];
        assert!(latest_matching(&summaries, "the founder").is_some());
        assert!(latest_matching(&summaries, "nobody").is_none());
    }
}
