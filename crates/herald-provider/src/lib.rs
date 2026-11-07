//! Provider capability boundary.
//!
//! A provider exposes a small, explicit set of operations. It never exposes
//! "click anything", "run JavaScript", or arbitrary page control to the planner.

pub mod fake;
pub mod imap;
pub mod maildir;
pub mod x;

pub(crate) mod mail;

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
///
/// `client_token` is the action id: the stable idempotency key for exactly one
/// delivery attempt. Providers that store sent messages use it to recognize a
/// retry instead of delivering the same reply twice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedAction {
    pub provider: ProviderId,
    pub thread_id: ThreadId,
    pub body: String,
    pub body_hash: String,
    pub prepared_at_ms: i64,
    /// Stable idempotency key for this attempt (the action id).
    pub client_token: String,
}

impl PreparedAction {
    #[must_use]
    pub fn new(
        provider: ProviderId,
        thread_id: ThreadId,
        body: impl Into<String>,
        client_token: impl Into<String>,
    ) -> Self {
        let body = body.into();
        Self {
            provider,
            thread_id,
            body_hash: body_hash(&body),
            body,
            prepared_at_ms: herald_core::time::now_ms(),
            client_token: client_token.into(),
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
    ///
    /// `false` means "delivered but not confirmed" (for example, SMTP accepted
    /// the message but the local Sent copy could not be written). Callers map
    /// that to `uncertain`, never to success.
    pub verified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProviderError {
    #[error("message or thread not found")]
    NotFound,
    #[error("the provider is not signed in")]
    NotAuthenticated,
    /// The provider exists but cannot perform this operation as configured.
    ///
    /// For a maildir this means the mailbox is read-only because no SMTP
    /// transport is configured.
    #[error("provider is not configured: {0}")]
    NotConfigured(String),
    #[error("provider UI did not match expectations: {0}")]
    UnexpectedLayout(String),
    #[error("provider rate limited the request")]
    RateLimited,
    /// Delivery may or may not have happened. Never blind-retry.
    #[error("provider request timed out")]
    Timeout,
    #[error("operation is not supported by this provider")]
    Unsupported,
    #[error("provider failure: {0}")]
    Other(String),
}

/// Reject an empty reply body at the provider boundary.
///
/// Every writable provider shares this fail-closed guard; the error message
/// is the only provider-specific part.
pub(crate) fn require_not_empty_body(
    body: &str,
    error_message: impl Into<String>,
) -> Result<(), ProviderError> {
    if body.trim().is_empty() {
        return Err(ProviderError::Other(error_message.into()));
    }
    Ok(())
}

/// The lowercased search needle, if any.
///
/// Blank searches are treated as no search rather than a filter that matches
/// nothing. Callers compute it once per query instead of repeating the
/// lowercasing for every message.
pub(crate) fn search_needle(query: &MessageQuery) -> Option<String> {
    query
        .search
        .as_deref()
        .map(str::to_ascii_lowercase)
        .filter(|value| !value.trim().is_empty())
}

/// Whether a message matches the search needle across sender, subject, and
/// body text. `None` matches everything.
pub(crate) fn matches_search(needle: Option<&String>, fields: &[&str]) -> bool {
    needle.is_none_or(|needle| {
        fields
            .iter()
            .any(|field| field.to_ascii_lowercase().contains(needle))
    })
}

/// The effective per-query limit. Zero means the default page size.
pub(crate) fn effective_limit(limit: usize) -> usize {
    if limit == 0 { 20 } else { limit }
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
        client_token: &str,
    ) -> Result<PreparedAction, ProviderError>;

    /// Send a previously prepared action.
    ///
    /// Implementations must be idempotent on `action.client_token`: a second
    /// call with the same token returns the receipt for the first delivery
    /// instead of sending again. They must re-resolve the thread and verify the
    /// result, and must not blind-retry on timeout.
    async fn send_reply(&self, action: &PreparedAction) -> Result<ExecutionReceipt, ProviderError>;

    /// Return the receipt for a message this client already delivered, if any.
    ///
    /// This is the reconciliation path for an `uncertain` send. It never
    /// sends.
    async fn find_sent(
        &self,
        thread: &ThreadId,
        client_token: &str,
    ) -> Result<Option<ExecutionReceipt>, ProviderError>;
}
