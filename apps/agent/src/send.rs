//! Execution of validated actions, the approval queue, and reconciliation.
//!
//! The rules of this module:
//! - reads and drafts are idempotent and local;
//! - a send is created as an approval, never executed by `ask`;
//! - `approve` re-resolves the target and re-checks the body hash;
//! - a timeout becomes `uncertain`, which is reconciled, never retried.

use herald_core::intent::DraftLength;
use herald_core::{
    Action, ActionState, DiffSegment, MessageQuery, MessageSummary, ProviderId, ReadAction,
    ReadOperation, SendAction, Target, Thread, ThreadId, body_hash, time::now_ms, word_diff,
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
        /// Deterministic length stats of the prepared body, in words and
        /// characters — the observable signal behind `[draft] length`.
        words: usize,
        chars: usize,
        /// The tone and length hints that shaped the prompt (from `[draft]`
        /// config), echoed so the caller can verify they applied.
        tone: Option<String>,
        length: DraftLength,
    },
    /// A planned read for a provider that has no adapter in this build.
    Deferred { provider: ProviderId },
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
            Self::Draft { body_hash, .. } => format!("draft {body_hash} prepared locally"),
            Self::Deferred { provider } => format!("no {provider} adapter; plan deferred"),
        }
    }
}

/// Deterministic length stats of a proposed body: whitespace-separated words
/// and characters. Used for the `[draft] length` feedback loop — the prompt
/// guides, these numbers report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyStats {
    pub words: usize,
    pub chars: usize,
}

#[must_use]
pub fn body_stats(body: &str) -> BodyStats {
    BodyStats {
        words: body.split_whitespace().count(),
        chars: body.chars().count(),
    }
}

/// Added/removed word counts of a diff, for one-line summaries.
#[must_use]
pub fn diff_counts(segments: &[DiffSegment]) -> (usize, usize) {
    let added: usize = segments
        .iter()
        .filter(|segment| segment.kind == herald_core::DiffKind::Add)
        .map(|segment| segment.text.split_whitespace().count())
        .sum();
    let removed: usize = segments
        .iter()
        .filter(|segment| segment.kind == herald_core::DiffKind::Del)
        .map(|segment| segment.text.split_whitespace().count())
        .sum();
    (added, removed)
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
    /// Word-level diff of `body` against the last message in the resolved
    /// thread, best-effort: `None` when the thread could not be read or the
    /// texts were empty/too large. A review aid, never a gate on sending —
    /// a failed fetch never blocks queueing.
    pub diff: Option<Vec<DiffSegment>>,
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
        Action::Draft(draft) => {
            let stats = body_stats(&draft.body);
            Ok(ExecutionOutcome::Draft {
                body_hash: body_hash(&draft.body),
                words: stats.words,
                chars: stats.chars,
                tone: app.config.draft.tone.clone(),
                length: app.config.draft.length,
            })
        }
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
            let Some(terms) = search_terms(&read.target) else {
                // validate_plan refuses the legacy timeline target, so this is
                // unreachable in practice; an empty filter would match
                // everything instead of refusing to guess.
                return Err(AppError::new(
                    "invalid_state",
                    "the timeline target was removed; nothing can be searched for",
                ));
            };
            let query = MessageQuery {
                search: Some(terms),
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
        Target::Timeline => {
            // The timeline target survives only so legacy approvals decode.
            // Every path that acts on it fails closed.
            Err(AppError::new(
                "invalid_state",
                "the timeline target was removed; nothing was sent",
            ))
        }
        Target::Description { description } => {
            let query = MessageQuery {
                search: Some(strip_search_prefix(description)),
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

/// Resolve a target and read its thread for draft context.
///
/// This only reads: it is best-effort and returns `None` on any failure, so a
/// draft can still be produced without context rather than failing the
/// command. The target is validated first, so unvalidated planner output never
/// reaches a provider.
pub async fn thread_for_draft(
    provider: &dyn CommunicationProvider,
    target: &Target,
) -> Option<Thread> {
    if herald_core::validate_target(target).is_err() {
        return None;
    }
    match target {
        Target::Thread { thread_id } => provider.get_thread(thread_id).await.ok(),
        Target::Description { .. } | Target::LatestFrom { .. } => {
            let resolved = resolve_thread(provider, target).await.ok()?;
            provider.get_thread(&resolved).await.ok()
        }
        Target::Timeline => None,
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

/// Search terms for a target, or `None` for the removed timeline target.
/// `None` is an error, never an empty filter: an empty filter would match
/// everything instead of refusing to guess.
fn search_terms(target: &Target) -> Option<String> {
    match target {
        Target::Description { description } => Some(strip_search_prefix(description)),
        Target::LatestFrom { sender } => Some(strip_sender_needle(sender)),
        Target::Thread { thread_id } => Some(thread_id.as_str().to_string()),
        Target::Timeline => None,
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
    let without_the = if matches_prefix(text, "the ") {
        text[4..].trim()
    } else {
        text
    };
    strip_time_suffix(without_the).to_string()
}

/// Drop trailing natural-language time phrases ("last week", "yesterday",
/// ...). Providers search sender/subject/body substrings and cannot filter
/// by date without an API change, so a time phrase must not become the
/// needle: "last week" alone becomes the empty (unfiltered recent) needle,
/// and "Sam last week" becomes "Sam".
///
/// These are phrases, not query syntax. Only the time expressions in this
/// list are treated as boilerplate.
fn strip_time_suffix(text: &str) -> &str {
    const SUFFIXES: [&str; 7] = [
        "last week",
        "last month",
        "last year",
        "this week",
        "this month",
        "yesterday",
        "today",
    ];
    let mut current = text.trim().trim_end_matches('.').trim();
    loop {
        let mut cut = None;
        for suffix in SUFFIXES {
            if current.eq_ignore_ascii_case(suffix) {
                return "";
            }
            if current.len() > suffix.len()
                && current
                    .get(current.len() - suffix.len()..)
                    .is_some_and(|tail| tail.eq_ignore_ascii_case(suffix))
                && current
                    .get(current.len() - suffix.len() - 1..current.len() - suffix.len())
                    .is_some_and(|separator| separator == " ")
            {
                cut = Some(current.len() - suffix.len());
                break;
            }
        }
        let Some(index) = cut else {
            return current;
        };
        current = current[..index].trim();
    }
}

/// Needle for a `LatestFrom` target. Sender/target resolution already cuts
/// "Sam" at " about ", so a search for "from Sam about the lease" and one
/// for "about the lease from Sam" both route forward as "Sam"; only a
/// trailing time phrase ("Sam last week") would otherwise become a needle
/// that matches nothing.
fn strip_sender_needle(sender: &str) -> String {
    let trimmed = sender.trim().trim_end_matches('.').trim();
    let without_the = if matches_prefix(trimmed, "the ") {
        trimmed[4..].trim()
    } else {
        trimmed
    };
    strip_time_suffix(without_the).to_string()
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
        // The timeline target is unreachable after validation.
        Action::Send(_) => "send_reply",
    }
}

/// Turn a send into a pending approval. Never sends.
pub async fn queue_send(
    app: &App,
    action: &SendAction,
    action_id: &str,
    draft: Option<&str>,
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

    // Record what the model proposed, before the human decided. Hashes and
    // word counts only: the acceptance metric never becomes a body store.
    if let Some(draft) = draft {
        let (added, removed) = if draft == action.body {
            (0, 0)
        } else {
            diff_counts(&word_diff(draft, &action.body))
        };
        app.store.record_draft_provenance(
            action_id,
            &body_hash(draft),
            i64::try_from(added).unwrap_or(i64::MAX),
            i64::try_from(removed).unwrap_or(i64::MAX),
        )?;
    }

    // Review aid: diff the proposed body against the last message in the
    // resolved thread so the reviewer sees what the reply adds on top of the
    // conversation. Best-effort by design — a thread that cannot be read
    // (or an empty/oversized pair) yields no diff, never a failed queue.
    let diff = match provider.get_thread(&resolved).await {
        Ok(thread) => thread
            .messages
            .last()
            .map(|message| word_diff(message.body.as_ref(), &action.body))
            .filter(|segments| !segments.is_empty()),
        Err(error) => {
            tracing::warn!(%error, "diff fetch failed; queuing without a diff view");
            None
        }
    };

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
        diff: diff.clone(),
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
        diff,
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
    )
    .await
}

/// The one path from a prepared action to a verified receipt.
async fn prepare_and_send(
    app: &App,
    provider: &dyn CommunicationProvider,
    action_id: &str,
    thread: &ThreadId,
    body: &str,
    expected_hash: &str,
) -> Result<SendOutcome, AppError> {
    let prepared = match provider.prepare_reply(thread, body, action_id).await {
        Ok(prepared) => prepared,
        Err(error) => {
            app.store
                .transition_action(action_id, ActionState::Failed)?;
            app.store.delete_approval(action_id)?;
            audit(app, Some(action_id), "failed", error.to_string())?;
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
            "provider changed the body; nothing was sent".to_string(),
        )?;
        return Err(AppError::conflict(
            "the provider changed the body; nothing was sent",
        ));
    }

    match provider.send_reply(&prepared).await {
        Ok(receipt) => finish_send(app, action_id, &receipt),
        Err(ProviderError::Timeout) => {
            app.store
                .transition_action(action_id, ActionState::Uncertain)?;
            app.store.delete_approval(action_id)?;
            audit(
                app,
                Some(action_id),
                "uncertain",
                "delivery timed out; reconcile before doing anything else".to_string(),
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
            audit(app, Some(action_id), "failed", error.to_string())?;
            Err(error.into())
        }
    }
}

fn finish_send(
    app: &App,
    action_id: &str,
    receipt: &ExecutionReceipt,
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
                "sent message {} to thread {}",
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
            "delivered but not confirmed locally; reconcile before retrying".to_string(),
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
    fn sender_and_topic_orderings_already_route_as_sender() {
        // `rules::extract_target` resolves "from Sam" (cutting "Sam" at
        // " about ") to `LatestFrom("Sam")` in both orderings, so the search
        // needle is already "Sam" and no new parsing is needed for those.
        let target = Target::LatestFrom {
            sender: "Sam".to_string(),
        };
        assert_eq!(search_terms(&target), Some("Sam".to_string()));
        assert_eq!(
            search_terms(&Target::LatestFrom {
                sender: "Sam last week".to_string(),
            }),
            Some("Sam".to_string())
        );
        assert_eq!(
            search_terms(&Target::LatestFrom {
                sender: "the founder".to_string(),
            }),
            Some("founder".to_string())
        );
    }

    #[test]
    fn trailing_time_phrases_do_not_become_the_needle() {
        assert_eq!(strip_search_prefix("Search for messages last week"), "");
        assert_eq!(
            strip_search_prefix("Search for messages about the lease last week"),
            "lease"
        );
        assert_eq!(
            strip_search_prefix("Search for messages about the lease."),
            "lease"
        );
        assert_eq!(
            search_terms(&Target::Description {
                description: "Search for messages last week".to_string(),
            }),
            Some(String::new())
        );
        assert_eq!(
            search_terms(&Target::Description {
                description: "Search for messages about the lease last week".to_string(),
            }),
            Some("lease".to_string())
        );
        assert_eq!(
            search_terms(&Target::LatestFrom {
                sender: "Sam last week".to_string(),
            }),
            Some("Sam".to_string())
        );
        assert_eq!(
            search_terms(&Target::LatestFrom {
                sender: "Sam yesterday".to_string(),
            }),
            Some("Sam".to_string())
        );
    }

    #[test]
    fn body_stats_count_words_and_chars() {
        assert_eq!(
            body_stats("Thanks, Thursday works."),
            BodyStats {
                words: 3,
                chars: 23
            }
        );
        assert_eq!(
            body_stats("  one   two\nthree "),
            BodyStats {
                words: 3,
                chars: 18
            }
        );
        assert_eq!(body_stats(""), BodyStats { words: 0, chars: 0 });
    }

    #[test]
    fn diff_counts_split_added_and_removed_words() {
        let segments = word_diff("thanks for the update", "thanks for the lease");
        assert_eq!(diff_counts(&segments), (1, 1));
        assert_eq!(diff_counts(&[]), (0, 0));
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
