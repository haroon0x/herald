//! Deterministic in-memory provider used by the MVP shell and integration tests.
//!
//! The fake provider is intentionally simple, but it enforces the same contract
//! as a real adapter: stable ids, read-only prepare, verified send, and explicit
//! failure modes.

use std::collections::BTreeMap;
use std::sync::Mutex;

use async_trait::async_trait;

use herald_core::{
    MessageId, MessageQuery, MessageSummary, ProviderId, ProviderMessage, Thread, ThreadId,
    Untrusted, time::now_ms,
};

use crate::{
    CommunicationProvider, ExecutionReceipt, PreparedAction, ProviderError, ProviderHealth,
};

#[derive(Debug, Clone)]
struct FakeThread {
    id: ThreadId,
    subject: String,
    messages: Vec<ProviderMessage>,
}

/// In-memory provider with a small seeded inbox.
#[derive(Debug)]
pub struct FakeProvider {
    threads: Mutex<BTreeMap<String, FakeThread>>,
    /// When set, the next send fails with this error after "reaching" the
    /// provider, which exercises the `uncertain` path.
    fail_next_send: Mutex<Option<ProviderError>>,
    signed_in: Mutex<bool>,
}

impl FakeProvider {
    #[must_use]
    pub fn new() -> Self {
        let now = now_ms();
        let mut threads = BTreeMap::new();

        let founder = FakeThread {
            id: ThreadId::new("t-founder"),
            subject: "Interview next steps".into(),
            messages: vec![
                ProviderMessage {
                    id: MessageId::new("m-founder-1"),
                    thread_id: ThreadId::new("t-founder"),
                    sender: "Alex Founder <alex@example.com>".into(),
                    subject: "Interview next steps".into(),
                    body: Untrusted(
                        "Great speaking with you. Are you still interested? Let me know.".into(),
                    ),
                    timestamp_ms: now - 3_600_000,
                    unread: true,
                },
                ProviderMessage {
                    id: MessageId::new("m-founder-2"),
                    thread_id: ThreadId::new("t-founder"),
                    sender: "Alex Founder <alex@example.com>".into(),
                    subject: "Interview next steps".into(),
                    body: Untrusted("Following up on the above.".into()),
                    timestamp_ms: now - 1_800_000,
                    unread: true,
                },
            ],
        };

        let sam = FakeThread {
            id: ThreadId::new("t-sam"),
            subject: "Lunch?".into(),
            messages: vec![ProviderMessage {
                id: MessageId::new("m-sam-1"),
                thread_id: ThreadId::new("t-sam"),
                sender: "Sam Lee <sam@example.com>".into(),
                subject: "Lunch?".into(),
                body: Untrusted("Free on Thursday?".into()),
                timestamp_ms: now - 7_200_000,
                unread: false,
            }],
        };

        threads.insert(founder.id.0.clone(), founder);
        threads.insert(sam.id.0.clone(), sam);

        Self {
            threads: Mutex::new(threads),
            fail_next_send: Mutex::new(None),
            signed_in: Mutex::new(true),
        }
    }

    pub fn sign_out(&self) {
        *self.signed_in.lock().unwrap() = false;
    }

    pub fn fail_next_send(&self, error: ProviderError) {
        *self.fail_next_send.lock().unwrap() = Some(error);
    }

    fn ensure_signed_in(&self) -> Result<(), ProviderError> {
        if *self.signed_in.lock().unwrap() {
            Ok(())
        } else {
            Err(ProviderError::NotAuthenticated)
        }
    }
}

impl Default for FakeProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CommunicationProvider for FakeProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Fake
    }

    async fn health(&self) -> ProviderHealth {
        if *self.signed_in.lock().unwrap() {
            ProviderHealth::Ready
        } else {
            ProviderHealth::NotAuthenticated
        }
    }

    async fn list_recent(
        &self,
        query: &MessageQuery,
    ) -> Result<Vec<MessageSummary>, ProviderError> {
        self.ensure_signed_in()?;
        let threads = self.threads.lock().unwrap();

        let mut summaries: Vec<MessageSummary> = threads
            .values()
            .flat_map(|thread| thread.messages.iter())
            .filter(|message| !query.unread_only || message.unread)
            .filter(|message| {
                query.search.as_deref().is_none_or(|needle| {
                    let needle = needle.to_lowercase();
                    message.subject.to_lowercase().contains(&needle)
                        || message.sender.to_lowercase().contains(&needle)
                        || message.body.as_ref().to_lowercase().contains(&needle)
                })
            })
            .map(|message| MessageSummary {
                id: message.id.clone(),
                thread_id: message.thread_id.clone(),
                sender: message.sender.clone(),
                subject: message.subject.clone(),
                timestamp_ms: message.timestamp_ms,
                unread: message.unread,
            })
            .collect();

        summaries.sort_by_key(|summary| std::cmp::Reverse(summary.timestamp_ms));
        let limit = if query.limit == 0 { 20 } else { query.limit };
        summaries.truncate(limit);
        Ok(summaries)
    }

    async fn get_thread(&self, id: &ThreadId) -> Result<Thread, ProviderError> {
        self.ensure_signed_in()?;
        let threads = self.threads.lock().unwrap();
        let thread = threads.get(id.as_str()).ok_or(ProviderError::NotFound)?;

        Ok(Thread {
            id: thread.id.clone(),
            provider: ProviderId::Fake,
            subject: thread.subject.clone(),
            messages: thread.messages.clone(),
        })
    }

    async fn prepare_reply(
        &self,
        id: &ThreadId,
        body: &str,
    ) -> Result<PreparedAction, ProviderError> {
        self.ensure_signed_in()?;
        if body.trim().is_empty() {
            return Err(ProviderError::Other("empty reply body".into()));
        }
        let threads = self.threads.lock().unwrap();
        if !threads.contains_key(id.as_str()) {
            return Err(ProviderError::NotFound);
        }
        Ok(PreparedAction::new(ProviderId::Fake, id.clone(), body))
    }

    async fn send_reply(&self, action: &PreparedAction) -> Result<ExecutionReceipt, ProviderError> {
        self.ensure_signed_in()?;

        // Simulate a send that reached the provider but whose response was lost.
        if let Some(error) = self.fail_next_send.lock().unwrap().take() {
            return Err(error);
        }

        let mut threads = self.threads.lock().unwrap();
        let thread = threads
            .get_mut(action.thread_id.as_str())
            .ok_or(ProviderError::NotFound)?;

        let message_id = MessageId::new(format!("m-sent-{}", action.prepared_at_ms));
        let now = now_ms();
        thread.messages.push(ProviderMessage {
            id: message_id.clone(),
            thread_id: action.thread_id.clone(),
            sender: "me".into(),
            subject: thread.subject.clone(),
            body: Untrusted(action.body.clone()),
            timestamp_ms: now,
            unread: false,
        });

        // Re-read the thread to prove the message is really there.
        let verified = thread
            .messages
            .iter()
            .any(|message| message.id == message_id && message.body.as_ref() == action.body);

        Ok(ExecutionReceipt {
            provider: ProviderId::Fake,
            thread_id: action.thread_id.clone(),
            provider_message_id: message_id,
            body_hash: action.body_hash.clone(),
            sent_at_ms: now,
            verified,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn lists_unread_only() {
        let provider = FakeProvider::new();
        let unread = provider
            .list_recent(&MessageQuery::unread(10))
            .await
            .unwrap();
        assert_eq!(unread.len(), 2);
        assert!(unread.iter().all(|message| message.unread));
    }

    #[tokio::test]
    async fn sends_and_verifies() {
        let provider = FakeProvider::new();
        let prepared = provider
            .prepare_reply(&ThreadId::new("t-founder"), "Thanks, I'm interested.")
            .await
            .unwrap();
        let receipt = provider.send_reply(&prepared).await.unwrap();
        assert!(receipt.verified);
    }

    #[tokio::test]
    async fn failed_send_does_not_duplicate() {
        let provider = FakeProvider::new();
        let prepared = provider
            .prepare_reply(&ThreadId::new("t-sam"), "yes")
            .await
            .unwrap();
        provider.fail_next_send(ProviderError::Timeout);
        assert!(provider.send_reply(&prepared).await.is_err());
        let receipt = provider.send_reply(&prepared).await.unwrap();
        assert!(receipt.verified);
    }

    #[tokio::test]
    async fn signed_out_provider_refuses_writes() {
        let provider = FakeProvider::new();
        provider.sign_out();
        assert_eq!(provider.health().await, ProviderHealth::NotAuthenticated);
        assert_eq!(
            provider
                .prepare_reply(&ThreadId::new("t-sam"), "hi")
                .await
                .unwrap_err(),
            ProviderError::NotAuthenticated
        );
    }
}
