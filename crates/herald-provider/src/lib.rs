//! Provider capability boundary.
//!
//! A provider exposes a small, explicit set of operations. It never exposes
//! "click anything", "run JavaScript", or arbitrary page control to the planner.

pub mod fake;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use herald_core::{
    MessageId, MessageQuery, MessageSummary, ProviderId, Thread, ThreadId, body_hash,
};

/// Result of a provider health check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderHealth {
    Ready,
    NotAuthenticated,
    Unavailable(String),
}

/// A provider-side draft that has not been sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedAction {
    pub provider: ProviderId,
    pub thread_id: ThreadId,
    pub body: String,
    pub body_hash: String,
    pub prepared_at_ms: i64,
}

impl PreparedAction {
    #[must_use]
    pub fn new(provider: ProviderId, thread_id: ThreadId, body: impl Into<String>) -> Self {
        let body = body.into();
        Self {
            provider,
            thread_id,
            body_hash: body_hash(&body),
            body,
            prepared_at_ms: herald_core::time::now_ms(),
        }
    }
}

/// Proof that a write action happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionReceipt {
    pub provider: ProviderId,
    pub thread_id: ThreadId,
    pub provider_message_id: MessageId,
    pub body_hash: String,
    pub sent_at_ms: i64,
    /// The adapter must re-read the thread and confirm the message is present.
    pub verified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProviderError {
    #[error("message or thread not found")]
    NotFound,
    #[error("the dedicated profile is not signed in")]
    NotAuthenticated,
    #[error("provider UI did not match expectations: {0}")]
    UnexpectedLayout(String),
    #[error("provider rate limited the request")]
    RateLimited,
    #[error("provider request timed out")]
    Timeout,
    #[error("operation is not supported by this provider")]
    Unsupported,
    #[error("provider failure: {0}")]
    Other(String),
}

/// The complete capability surface a provider may expose.
#[async_trait]
pub trait CommunicationProvider: Send + Sync {
    fn id(&self) -> ProviderId;

    async fn health(&self) -> ProviderHealth;

    async fn list_recent(&self, query: &MessageQuery)
    -> Result<Vec<MessageSummary>, ProviderError>;

    async fn get_thread(&self, id: &ThreadId) -> Result<Thread, ProviderError>;

    /// Prepare a reply without sending it. Must not mutate the account.
    async fn prepare_reply(
        &self,
        id: &ThreadId,
        body: &str,
    ) -> Result<PreparedAction, ProviderError>;

    /// Send a previously prepared action. Implementations must re-resolve the
    /// thread and verify the result; they must not blind-retry on timeout.
    async fn send_reply(&self, action: &PreparedAction) -> Result<ExecutionReceipt, ProviderError>;
}
