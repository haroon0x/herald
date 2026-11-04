//! Deterministic in-memory provider used by the MVP shell and integration tests.
//!
//! The fake provider is intentionally simple, but it enforces the same contract
//! as a real adapter: stable ids, read-only prepare, verified idempotent send,
//! and explicit failure modes.
//!
//! [`FakeProvider::with_state`] adds a small best-effort JSON ledger so a
//! separate CLI process can reconcile a send that a previous process made.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
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

type TokenLedger = BTreeMap<String, ExecutionReceipt>;

/// In-memory provider with a small seeded inbox.
#[derive(Debug)]
pub struct FakeProvider {
    threads: Mutex<BTreeMap<String, FakeThread>>,
    /// Client tokens this client already delivered, keyed by the action id.
    sent: Mutex<TokenLedger>,
    /// When set, the next send fails before delivery. This exercises `failed`.
    fail_next_send: Mutex<Option<ProviderError>>,
    /// When set, the next send fails after delivery and after the token is
    /// recorded. This exercises `uncertain` and the `find_sent` recovery path.
    fail_next_send_after_delivery: Mutex<Option<ProviderError>>,
    signed_in: Mutex<bool>,
    /// Optional best-effort ledger shared between CLI processes.
    state_path: Option<PathBuf>,
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
            sent: Mutex::new(TokenLedger::new()),
            fail_next_send: Mutex::new(None),
            fail_next_send_after_delivery: Mutex::new(None),
            signed_in: Mutex::new(true),
            state_path: None,
        }
    }

    /// Construct a provider that persists its sent-token ledger at `path`.
    ///
    /// The ledger is only the `client_token -> ExecutionReceipt` map. Loading a
    /// missing, unreadable, or malformed file silently falls back to an empty
    /// ledger: reads and sends must not fail because of demo state.
    #[must_use]
    pub fn with_state(path: PathBuf) -> Self {
        let sent = load_ledger(&path).unwrap_or_default();
        Self {
            sent: Mutex::new(sent),
            state_path: Some(path),
            ..Self::new()
        }
    }

    pub fn sign_out(&self) {
        *self.signed_in.lock().unwrap() = false;
    }

    /// Fail the next send before delivery. The token is not recorded, so a
    /// retry delivers exactly once.
    pub fn fail_next_send(&self, error: ProviderError) {
        *self.fail_next_send.lock().unwrap() = Some(error);
    }

    /// Fail the next send after delivery and after the token is recorded.
    ///
    /// The caller sees an error (typically mapped to `uncertain`), but
    /// [`CommunicationProvider::find_sent`] still recovers the receipt.
    pub fn fail_next_send_after_delivery(&self, error: ProviderError) {
        *self.fail_next_send_after_delivery.lock().unwrap() = Some(error);
    }

    fn ensure_signed_in(&self) -> Result<(), ProviderError> {
        if *self.signed_in.lock().unwrap() {
            Ok(())
        } else {
            Err(ProviderError::NotAuthenticated)
        }
    }

    fn recorded_receipt(&self, client_token: &str) -> Option<ExecutionReceipt> {
        self.sent.lock().unwrap().get(client_token).cloned()
    }

    /// Best-effort persistence. A failed write never changes the send result
    /// and never panics.
    fn save_ledger(&self) {
        let Some(path) = self.state_path.as_deref() else {
            return;
        };
        let ledger = self.sent.lock().unwrap().clone();
        let _ = write_ledger(path, &ledger);
    }
}

impl Default for FakeProvider {
    fn default() -> Self {
        Self::new()
    }
}

fn load_ledger(path: &Path) -> Option<TokenLedger> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_ledger(path: &Path, ledger: &TokenLedger) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(ledger)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, bytes)?;
    fs::rename(temporary, path)
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
        client_token: &str,
    ) -> Result<PreparedAction, ProviderError> {
        self.ensure_signed_in()?;
        if body.trim().is_empty() {
            return Err(ProviderError::Other("empty reply body".into()));
        }
        let threads = self.threads.lock().unwrap();
        if !threads.contains_key(id.as_str()) {
            return Err(ProviderError::NotFound);
        }
        Ok(PreparedAction::new(
            ProviderId::Fake,
            id.clone(),
            body,
            client_token,
        ))
    }

    async fn send_reply(&self, action: &PreparedAction) -> Result<ExecutionReceipt, ProviderError> {
        self.ensure_signed_in()?;

        // Idempotency: the token is recorded exactly once.
        if let Some(receipt) = self.recorded_receipt(&action.client_token) {
            return Ok(receipt);
        }

        // Fail-before-delivery: nothing is recorded, so a retry is safe.
        if let Some(error) = self.fail_next_send.lock().unwrap().take() {
            return Err(error);
        }

        let message_id = MessageId::new(format!("m-sent-{}", action.client_token));
        let now = now_ms();
        let receipt = {
            let mut threads = self.threads.lock().unwrap();
            let thread = threads
                .get_mut(action.thread_id.as_str())
                .ok_or(ProviderError::NotFound)?;

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

            ExecutionReceipt {
                provider: ProviderId::Fake,
                thread_id: action.thread_id.clone(),
                provider_message_id: message_id,
                body_hash: action.body_hash.clone(),
                sent_at_ms: now,
                verified,
            }
        };

        self.sent
            .lock()
            .unwrap()
            .insert(action.client_token.clone(), receipt.clone());
        self.save_ledger();

        // Delivered, recorded, and reconciled by `find_sent`; the caller still
        // sees the failure so the action becomes `uncertain`, never success.
        if let Some(error) = self.fail_next_send_after_delivery.lock().unwrap().take() {
            return Err(error);
        }

        Ok(receipt)
    }

    async fn find_sent(
        &self,
        _thread: &ThreadId,
        client_token: &str,
    ) -> Result<Option<ExecutionReceipt>, ProviderError> {
        self.ensure_signed_in()?;
        Ok(self.recorded_receipt(client_token))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prepared(provider: &FakeProvider, thread: &str, body: &str, token: &str) -> PreparedAction {
        // Tests are sync over the async trait; call the constructor directly.
        let _ = provider;
        PreparedAction::new(ProviderId::Fake, ThreadId::new(thread), body, token)
    }

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
            .prepare_reply(
                &ThreadId::new("t-founder"),
                "Thanks, I'm interested.",
                "token-1",
            )
            .await
            .unwrap();
        let receipt = provider.send_reply(&prepared).await.unwrap();
        assert!(receipt.verified);
    }

    #[tokio::test]
    async fn same_token_sends_only_once() {
        let provider = FakeProvider::new();
        let prepared = prepared(&provider, "t-founder", "hello", "token-1");
        let first = provider.send_reply(&prepared).await.unwrap();
        let second = provider.send_reply(&prepared).await.unwrap();
        assert_eq!(first, second);

        let thread = provider
            .get_thread(&ThreadId::new("t-founder"))
            .await
            .unwrap();
        assert_eq!(
            thread
                .messages
                .iter()
                .filter(|message| message.id.as_str() == "m-sent-token-1")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn fail_before_delivery_then_retry_delivers_once() {
        let provider = FakeProvider::new();
        let prepared = provider
            .prepare_reply(&ThreadId::new("t-sam"), "yes", "token-2")
            .await
            .unwrap();
        provider.fail_next_send(ProviderError::Timeout);
        assert_eq!(
            provider.send_reply(&prepared).await.unwrap_err(),
            ProviderError::Timeout
        );
        assert!(
            provider
                .find_sent(&prepared.thread_id, "token-2")
                .await
                .unwrap()
                .is_none()
        );

        let receipt = provider.send_reply(&prepared).await.unwrap();
        assert!(receipt.verified);
        let thread = provider.get_thread(&ThreadId::new("t-sam")).await.unwrap();
        assert_eq!(
            thread
                .messages
                .iter()
                .filter(|message| message.id.as_str() == "m-sent-token-2")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn fail_after_delivery_is_recovered_by_find_sent() {
        let provider = FakeProvider::new();
        let prepared = provider
            .prepare_reply(&ThreadId::new("t-sam"), "yes", "token-3")
            .await
            .unwrap();
        provider.fail_next_send_after_delivery(ProviderError::Timeout);
        assert_eq!(
            provider.send_reply(&prepared).await.unwrap_err(),
            ProviderError::Timeout
        );

        let found = provider
            .find_sent(&prepared.thread_id, "token-3")
            .await
            .unwrap()
            .expect("delivered message must be recoverable");
        assert!(found.verified);
        assert_eq!(found.body_hash, prepared.body_hash);

        // A retry after the ambiguous failure must not deliver twice.
        assert_eq!(provider.send_reply(&prepared).await.unwrap(), found);
        let thread = provider.get_thread(&ThreadId::new("t-sam")).await.unwrap();
        assert_eq!(
            thread
                .messages
                .iter()
                .filter(|message| message.id.as_str() == "m-sent-token-3")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn find_sent_returns_none_for_unknown_token() {
        let provider = FakeProvider::new();
        assert!(
            provider
                .find_sent(&ThreadId::new("t-sam"), "nope")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn signed_out_provider_refuses_writes() {
        let provider = FakeProvider::new();
        provider.sign_out();
        assert_eq!(provider.health().await, ProviderHealth::NotAuthenticated);
        assert_eq!(
            provider
                .prepare_reply(&ThreadId::new("t-sam"), "hi", "token-4")
                .await
                .unwrap_err(),
            ProviderError::NotAuthenticated
        );
    }

    #[tokio::test]
    async fn ledger_survives_a_new_provider_instance() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("demo-provider.json");

        let receipt = {
            let provider = FakeProvider::with_state(path.clone());
            let prepared = provider
                .prepare_reply(&ThreadId::new("t-founder"), "persisted", "token-5")
                .await
                .unwrap();
            provider.send_reply(&prepared).await.unwrap()
        };

        let provider = FakeProvider::with_state(path);
        let found = provider
            .find_sent(&ThreadId::new("t-founder"), "token-5")
            .await
            .unwrap()
            .expect("ledger must survive the process boundary");
        assert_eq!(found, receipt);

        let prepared = prepared(&provider, "t-founder", "persisted", "token-5");
        assert_eq!(provider.send_reply(&prepared).await.unwrap(), receipt);
        let thread = provider
            .get_thread(&ThreadId::new("t-founder"))
            .await
            .unwrap();
        // The fresh instance has fresh seeded threads; an early return from the
        // ledger must not append a new message there.
        assert_eq!(
            thread
                .messages
                .iter()
                .filter(|message| message.id.as_str() == "m-sent-token-5")
                .count(),
            0
        );
    }

    #[test]
    fn a_malformed_ledger_does_not_break_construction() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("demo-provider.json");
        fs::write(&path, b"not json").unwrap();
        let provider = FakeProvider::with_state(path);
        assert!(provider.recorded_receipt("token-5").is_none());
    }
}
