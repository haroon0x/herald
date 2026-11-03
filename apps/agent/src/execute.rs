//! Execution of validated read and draft actions.
//!
//! Sends never reach this module: they stop at the approval record and are
//! handled by the Phase 3 execution path. Everything here is read-only against
//! the provider and idempotent against local storage.

use anyhow::Result;

use herald_core::{
    Action, ActionState, MessageQuery, MessageSummary, ProviderId, ReadAction, ReadOperation,
    Target, Thread, ThreadId, time::now_ms,
};
use herald_provider::{CommunicationProvider, ProviderError};
use herald_storage::{AuditEvent, StorageError, Store, StoredMessage};

const LIST_LIMIT: usize = 20;
const RESOLVE_LIMIT: usize = 50;

#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    #[error("{0}")]
    Provider(#[from] ProviderError),
    #[error("storage error: {0}")]
    Storage(#[from] StorageError),
    #[error("no thread matches {0:?}")]
    UnresolvedTarget(String),
    #[error("action is not executable locally: {0}")]
    NotExecutable(&'static str),
}

/// What a completed action produced, for display by the CLI.
#[derive(Debug)]
pub enum ExecutionOutcome {
    Messages {
        summaries: Vec<MessageSummary>,
        new_count: usize,
    },
    Thread {
        thread: Thread,
        new_count: usize,
    },
    Draft {
        body_hash: String,
    },
}

/// Execute an allowed read or draft action and drive its state machine.
///
/// A provider failure ends in `failed` (never `succeeded`) and is reported to
/// the caller. Reads and drafts are safe to repeat because message persistence
/// dedupes by `(provider, message_id)` and drafts do not mutate the account.
pub async fn execute(
    action: &Action,
    provider: Option<&dyn CommunicationProvider>,
    store: &Store,
    action_id: &str,
) -> Result<ExecutionOutcome, ExecutionError> {
    if matches!(action, Action::Send(_)) {
        return Err(ExecutionError::NotExecutable(
            "sends must pass through the approval record",
        ));
    }

    store.transition_action(action_id, ActionState::Executing)?;
    store.append_audit(&AuditEvent {
        action_id: Some(action_id.to_string()),
        at_ms: now_ms(),
        status: "executing".into(),
        detail: format!("{} {}", action.provider(), action.target().describe()),
    })?;

    let result = match action {
        Action::Read(read) => execute_read(read, provider, store).await,
        Action::Draft(draft) => Ok(ExecutionOutcome::Draft {
            body_hash: herald_core::body_hash(&draft.body),
        }),
        Action::Send(_) => unreachable!("guarded above"),
    };

    match result {
        Ok(outcome) => {
            store.transition_action(action_id, ActionState::Succeeded)?;
            store.append_audit(&AuditEvent {
                action_id: Some(action_id.to_string()),
                at_ms: now_ms(),
                status: "succeeded".into(),
                detail: outcome.audit_detail(),
            })?;
            Ok(outcome)
        }
        Err(error) => {
            store.transition_action(action_id, ActionState::Failed)?;
            store.append_audit(&AuditEvent {
                action_id: Some(action_id.to_string()),
                at_ms: now_ms(),
                status: "failed".into(),
                detail: error.to_string(),
            })?;
            Err(error)
        }
    }
}

impl ExecutionOutcome {
    fn audit_detail(&self) -> String {
        match self {
            Self::Messages {
                summaries,
                new_count,
            } => format!("{} message(s), {new_count} new", summaries.len()),
            Self::Thread { thread, new_count } => {
                format!("{} message(s), {new_count} new", thread.messages.len())
            }
            Self::Draft { body_hash } => format!("draft {body_hash} prepared locally"),
        }
    }
}

async fn execute_read(
    read: &ReadAction,
    provider: Option<&dyn CommunicationProvider>,
    store: &Store,
) -> Result<ExecutionOutcome, ExecutionError> {
    let provider = provider.ok_or(ExecutionError::NotExecutable(
        "no provider adapter is implemented for this provider",
    ))?;

    match read.operation {
        ReadOperation::ListRecentMessages => {
            let summaries = provider
                .list_recent(&MessageQuery::recent(LIST_LIMIT))
                .await?;
            Ok(ExecutionOutcome::Messages {
                new_count: record_summaries(store, read.provider, &summaries)?,
                summaries,
            })
        }
        ReadOperation::ListUnreadMessages => {
            let summaries = provider
                .list_recent(&MessageQuery::unread(LIST_LIMIT))
                .await?;
            Ok(ExecutionOutcome::Messages {
                new_count: record_summaries(store, read.provider, &summaries)?,
                summaries,
            })
        }
        ReadOperation::SearchMessages => {
            let terms = search_terms(&read.target);
            let query = MessageQuery {
                search: Some(terms),
                limit: LIST_LIMIT,
                ..MessageQuery::default()
            };
            let summaries = provider.list_recent(&query).await?;
            Ok(ExecutionOutcome::Messages {
                new_count: record_summaries(store, read.provider, &summaries)?,
                summaries,
            })
        }
        ReadOperation::GetThread | ReadOperation::SummarizeThread => {
            let thread_id = resolve_thread(&read.target, provider).await?;
            let thread = provider.get_thread(&thread_id).await?;
            let new_count = record_thread(store, read.provider, &thread)?;
            Ok(ExecutionOutcome::Thread { thread, new_count })
        }
    }
}

/// Resolve a planner target against real provider data.
///
/// A description is not an identifier: this is the only step that turns one
/// into a thread id, and it fails instead of guessing when nothing matches.
async fn resolve_thread(
    target: &Target,
    provider: &dyn CommunicationProvider,
) -> Result<ThreadId, ExecutionError> {
    match target {
        Target::Thread { thread_id } => Ok(thread_id.clone()),
        Target::LatestFrom { sender } => {
            let summaries = provider
                .list_recent(&MessageQuery::recent(RESOLVE_LIMIT))
                .await?;
            latest_matching(&summaries, sender)
                .map(|summary| summary.thread_id.clone())
                .ok_or_else(|| ExecutionError::UnresolvedTarget(sender.clone()))
        }
        Target::Description { description } => {
            let query = MessageQuery {
                search: Some(search_terms(target)),
                limit: RESOLVE_LIMIT,
                ..MessageQuery::default()
            };
            let summaries = provider.list_recent(&query).await?;
            summaries
                .first()
                .map(|summary| summary.thread_id.clone())
                .ok_or_else(|| ExecutionError::UnresolvedTarget(description.clone()))
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
    }
}

/// Trim command boilerplate so a search does not look for the whole sentence.
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
        if text_matches_prefix(text, prefix) {
            text = &text[prefix.len()..];
            break;
        }
    }
    let text = text.trim();
    if text_matches_prefix(text, "the ") {
        return text[4..].trim().to_string();
    }
    text.to_string()
}

fn text_matches_prefix(text: &str, prefix: &str) -> bool {
    text.len() >= prefix.len()
        && text
            .get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

fn record_summaries(
    store: &Store,
    provider: ProviderId,
    summaries: &[MessageSummary],
) -> Result<usize, StorageError> {
    let mut new_count = 0;
    for summary in summaries {
        let stored = StoredMessage {
            provider,
            message_id: summary.id.clone(),
            thread_id: summary.thread_id.clone(),
            sender: summary.sender.clone(),
            subject: summary.subject.clone(),
            timestamp_ms: summary.timestamp_ms,
            unread: summary.unread,
        };
        if store.record_message(&stored)? {
            new_count += 1;
        }
    }
    Ok(new_count)
}

fn record_thread(
    store: &Store,
    provider: ProviderId,
    thread: &Thread,
) -> Result<usize, StorageError> {
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
        if store.record_message(&stored)? {
            new_count += 1;
        }
    }
    Ok(new_count)
}

/// Locate the only implemented adapter for a provider.
pub fn adapter(provider: ProviderId) -> Option<Box<dyn CommunicationProvider>> {
    match provider {
        ProviderId::Fake => Some(Box::new(herald_provider::fake::FakeProvider::new())),
        ProviderId::Gmail | ProviderId::X => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use herald_core::{DraftAction, ReadOperation, SendAction, ThreadId, body_hash, validate_plan};
    use herald_provider::fake::FakeProvider;
    use herald_storage::new_action;

    fn read_action(operation: ReadOperation, target: Target) -> Action {
        Action::Read(ReadAction {
            provider: ProviderId::Fake,
            operation,
            target,
        })
    }

    fn store_with_action(action_id: &str, action: &Action) -> Store {
        let store = Store::open_in_memory().unwrap();
        let record = new_action(
            action_id,
            action.provider(),
            "test",
            action.target().describe(),
            None,
        );
        store.create_action(&record).unwrap();
        store
    }

    #[tokio::test]
    async fn reading_records_messages_once() {
        let provider = FakeProvider::new();
        let action = read_action(
            ReadOperation::ListUnreadMessages,
            Target::Description {
                description: "unread".into(),
            },
        );
        let store = store_with_action("a1", &action);

        let first = execute(&action, Some(&provider), &store, "a1")
            .await
            .unwrap();
        match first {
            ExecutionOutcome::Messages {
                summaries,
                new_count,
            } => {
                assert_eq!(summaries.len(), 2);
                assert_eq!(new_count, 2);
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
        assert_eq!(store.message_count().unwrap(), 2);
        assert_eq!(
            store.recent_actions(1).unwrap()[0].state,
            ActionState::Succeeded
        );

        // A second scan is deduped: nothing is "new" again.
        let action2 = read_action(
            ReadOperation::ListUnreadMessages,
            Target::Description {
                description: "unread".into(),
            },
        );
        store
            .create_action(&new_action("a2", ProviderId::Fake, "test", "unread", None))
            .unwrap();
        let second = execute(&action2, Some(&provider), &store, "a2")
            .await
            .unwrap();
        match second {
            ExecutionOutcome::Messages { new_count, .. } => assert_eq!(new_count, 0),
            other => panic!("unexpected outcome: {other:?}"),
        }
        assert_eq!(store.message_count().unwrap(), 2);
    }

    #[tokio::test]
    async fn search_trims_command_boilerplate() {
        let provider = FakeProvider::new();
        let action = read_action(
            ReadOperation::SearchMessages,
            Target::Description {
                description: "Search for messages about the interview.".into(),
            },
        );
        let store = store_with_action("a-search", &action);

        let outcome = execute(&action, Some(&provider), &store, "a-search")
            .await
            .unwrap();
        match outcome {
            ExecutionOutcome::Messages { summaries, .. } => {
                assert_eq!(summaries.len(), 2);
                assert!(
                    summaries
                        .iter()
                        .all(|summary| summary.subject.to_lowercase().contains("interview"))
                );
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[tokio::test]
    async fn latest_from_resolves_to_a_real_thread() {
        let provider = FakeProvider::new();
        let action = read_action(
            ReadOperation::GetThread,
            Target::LatestFrom {
                sender: "the founder".into(),
            },
        );
        let store = store_with_action("a3", &action);

        let outcome = execute(&action, Some(&provider), &store, "a3")
            .await
            .unwrap();
        match outcome {
            ExecutionOutcome::Thread { thread, new_count } => {
                assert_eq!(thread.subject, "Interview next steps");
                assert_eq!(new_count, 2);
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[tokio::test]
    async fn unknown_sender_is_unresolved_not_guessed() {
        let provider = FakeProvider::new();
        let action = read_action(
            ReadOperation::GetThread,
            Target::LatestFrom {
                sender: "nobody".into(),
            },
        );
        let store = store_with_action("a4", &action);

        let error = execute(&action, Some(&provider), &store, "a4")
            .await
            .unwrap_err();
        assert!(matches!(error, ExecutionError::UnresolvedTarget(_)));
        assert_eq!(
            store.recent_actions(1).unwrap()[0].state,
            ActionState::Failed
        );
    }

    #[tokio::test]
    async fn provider_failure_ends_failed_not_succeeded() {
        let provider = FakeProvider::new();
        provider.sign_out();
        let action = read_action(
            ReadOperation::ListRecentMessages,
            Target::Description {
                description: "recent".into(),
            },
        );
        let store = store_with_action("a5", &action);

        let error = execute(&action, Some(&provider), &store, "a5")
            .await
            .unwrap_err();
        assert!(matches!(error, ExecutionError::Provider(_)));
        assert_eq!(
            store.recent_actions(1).unwrap()[0].state,
            ActionState::Failed
        );
    }

    #[tokio::test]
    async fn drafts_are_local_and_never_send() {
        let provider = FakeProvider::new();
        let action = Action::Draft(DraftAction {
            provider: ProviderId::Fake,
            target: Target::LatestFrom {
                sender: "the founder".into(),
            },
            body: "Thanks, I'm interested.".into(),
        });
        let store = store_with_action("a6", &action);

        let outcome = execute(&action, Some(&provider), &store, "a6")
            .await
            .unwrap();
        match outcome {
            ExecutionOutcome::Draft { body_hash: hash } => {
                assert_eq!(hash, body_hash("Thanks, I'm interested."));
            }
            other => panic!("unexpected outcome: {other:?}"),
        }

        let thread = provider
            .get_thread(&ThreadId::new("t-founder"))
            .await
            .unwrap();
        assert!(thread.messages.iter().all(|message| message.sender != "me"));
    }

    #[tokio::test]
    async fn sends_are_refused_by_the_executor() {
        let provider = FakeProvider::new();
        let action = Action::Send(SendAction {
            provider: ProviderId::Fake,
            target: Target::Thread {
                thread_id: ThreadId::new("t-founder"),
            },
            body: "hi".into(),
            body_hash: body_hash("hi"),
        });
        let store = store_with_action("a7", &action);

        let error = execute(&action, Some(&provider), &store, "a7")
            .await
            .unwrap_err();
        assert!(matches!(error, ExecutionError::NotExecutable(_)));
        assert_eq!(
            store.recent_actions(1).unwrap()[0].state,
            ActionState::Planned
        );
    }

    #[test]
    fn plans_still_validate_before_execution() {
        let plan: herald_core::ActionPlan = serde_json::from_str(
            r#"{
                "provider": "fake",
                "operation": "search_messages",
                "target": {"kind": "description", "description": "search for messages about the interview"}
            }"#,
        )
        .unwrap();
        assert!(validate_plan(plan).is_ok());
        assert_eq!(
            strip_search_prefix("Search for messages about the interview."),
            "interview"
        );
    }
}
