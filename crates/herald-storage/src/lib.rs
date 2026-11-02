//! SQLite state and audit log.
//!
//! The store is the source of truth for which messages have been seen and what
//! happened to every write action. The model never owns this state.

use std::path::Path;
use std::str::FromStr;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use herald_core::{ActionState, MessageId, MessageSummary, ProviderId, ThreadId, time::now_ms};

const SCHEMA: &str = include_str!("../schema.sql");
const SCHEMA_VERSION: i64 = 1;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unknown action state in database: {0}")]
    BadState(String),
    #[error("action {0} was not found")]
    ActionNotFound(String),
    #[error("illegal transition for action {action_id}: {source}")]
    IllegalTransition {
        action_id: String,
        #[source]
        source: herald_core::TransitionError,
    },
}

/// Message row as persisted locally. Bodies are intentionally not stored by
/// default; only identity and routing metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredMessage {
    pub provider: ProviderId,
    pub message_id: MessageId,
    pub thread_id: ThreadId,
    pub sender: String,
    pub subject: String,
    pub timestamp_ms: i64,
    pub unread: bool,
}

impl From<&MessageSummary> for StoredMessage {
    fn from(summary: &MessageSummary) -> Self {
        Self {
            provider: ProviderId::Fake,
            message_id: summary.id.clone(),
            thread_id: summary.thread_id.clone(),
            sender: summary.sender.clone(),
            subject: summary.subject.clone(),
            timestamp_ms: summary.timestamp_ms,
            unread: summary.unread,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionRecord {
    pub action_id: String,
    pub provider: ProviderId,
    pub operation: String,
    pub target: String,
    pub content_hash: Option<String>,
    pub state: ActionState,
    pub created_at_ms: i64,
    pub completed_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEvent {
    pub action_id: Option<String>,
    pub at_ms: i64,
    pub status: String,
    pub detail: String,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let conn = Connection::open(path)?;
        Self::from_connection(conn)
    }

    pub fn open_in_memory() -> Result<Self, StorageError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Self, StorageError> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version < SCHEMA_VERSION {
            conn.execute_batch(SCHEMA)?;
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        Ok(Self { conn })
    }

    /// Returns `true` when the message was not already known.
    pub fn record_message(&self, message: &StoredMessage) -> Result<bool, StorageError> {
        let inserted = self.conn.execute(
            "INSERT OR IGNORE INTO messages
                (provider, message_id, thread_id, sender, subject, timestamp_ms, unread, last_seen_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                message.provider.as_str(),
                message.message_id.as_str(),
                message.thread_id.as_str(),
                message.sender,
                message.subject,
                message.timestamp_ms,
                i64::from(message.unread),
                now_ms(),
            ],
        )?;

        if inserted == 0 {
            self.conn.execute(
                "UPDATE messages SET unread = ?3, last_seen_at = ?4
                 WHERE provider = ?1 AND message_id = ?2",
                params![
                    message.provider.as_str(),
                    message.message_id.as_str(),
                    i64::from(message.unread),
                    now_ms(),
                ],
            )?;
        }

        Ok(inserted == 1)
    }

    pub fn message_count(&self) -> Result<usize, StorageError> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))?;
        Ok(usize::try_from(count).unwrap_or(usize::MAX))
    }

    pub fn create_action(&self, action: &ActionRecord) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO actions
                (action_id, provider, operation, target, content_hash, state, created_at, completed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                action.action_id,
                action.provider.as_str(),
                action.operation,
                action.target,
                action.content_hash,
                action.state.as_str(),
                action.created_at_ms,
                action.completed_at_ms,
            ],
        )?;
        Ok(())
    }

    /// Move an action to `next`, rejecting illegal transitions.
    pub fn transition_action(
        &self,
        action_id: &str,
        next: ActionState,
    ) -> Result<ActionState, StorageError> {
        let current: String = self
            .conn
            .query_row(
                "SELECT state FROM actions WHERE action_id = ?1",
                params![action_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| StorageError::ActionNotFound(action_id.to_string()))?;

        let current = ActionState::from_str(&current).map_err(StorageError::BadState)?;
        let next = current
            .transition(next)
            .map_err(|source| StorageError::IllegalTransition {
                action_id: action_id.to_string(),
                source,
            })?;

        let completed_at = next.is_terminal().then(now_ms);
        self.conn.execute(
            "UPDATE actions SET state = ?2, completed_at = ?3 WHERE action_id = ?1",
            params![action_id, next.as_str(), completed_at],
        )?;
        Ok(next)
    }

    pub fn recent_actions(&self, limit: usize) -> Result<Vec<ActionRecord>, StorageError> {
        let mut statement = self.conn.prepare(
            "SELECT action_id, provider, operation, target, content_hash, state, created_at, completed_at
             FROM actions ORDER BY created_at DESC LIMIT ?1",
        )?;

        let rows =
            statement.query_map(params![i64::try_from(limit).unwrap_or(i64::MAX)], |row| {
                let provider: String = row.get(1)?;
                let state: String = row.get(5)?;
                Ok((
                    row.get::<_, String>(0)?,
                    provider,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    state,
                    row.get::<_, i64>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                ))
            })?;

        let mut actions = Vec::new();
        for row in rows {
            let (
                action_id,
                provider,
                operation,
                target,
                content_hash,
                state,
                created_at,
                completed,
            ) = row?;
            actions.push(ActionRecord {
                action_id,
                provider: ProviderId::from_str(&provider).map_err(StorageError::BadState)?,
                operation,
                target,
                content_hash,
                state: ActionState::from_str(&state).map_err(StorageError::BadState)?,
                created_at_ms: created_at,
                completed_at_ms: completed,
            });
        }
        Ok(actions)
    }

    pub fn append_audit(&self, event: &AuditEvent) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO audit_events (action_id, at_ms, status, detail)
             VALUES (?1, ?2, ?3, ?4)",
            params![event.action_id, event.at_ms, event.status, event.detail],
        )?;
        Ok(())
    }

    pub fn audit_count(&self) -> Result<usize, StorageError> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM audit_events", [], |row| row.get(0))?;
        Ok(usize::try_from(count).unwrap_or(usize::MAX))
    }

    pub fn set_preference(&self, key: &str, value: &str) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO preferences (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn get_preference(&self, key: &str) -> Result<Option<String>, StorageError> {
        Ok(self
            .conn
            .query_row(
                "SELECT value FROM preferences WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?)
    }
}

/// Convenience constructor for a freshly planned action.
#[must_use]
pub fn new_action(
    action_id: impl Into<String>,
    provider: ProviderId,
    operation: impl Into<String>,
    target: impl Into<String>,
    content_hash: Option<String>,
) -> ActionRecord {
    ActionRecord {
        action_id: action_id.into(),
        provider,
        operation: operation.into(),
        target: target.into(),
        content_hash,
        state: ActionState::Planned,
        created_at_ms: now_ms(),
        completed_at_ms: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(id: &str) -> StoredMessage {
        StoredMessage {
            provider: ProviderId::Fake,
            message_id: MessageId::new(id),
            thread_id: ThreadId::new("t1"),
            sender: "someone".into(),
            subject: "subject".into(),
            timestamp_ms: 1,
            unread: true,
        }
    }

    #[test]
    fn deduplicates_messages() {
        let store = Store::open_in_memory().unwrap();
        assert!(store.record_message(&message("m1")).unwrap());
        assert!(!store.record_message(&message("m1")).unwrap());
        assert_eq!(store.message_count().unwrap(), 1);
    }

    #[test]
    fn action_state_machine_is_enforced_in_storage() {
        let store = Store::open_in_memory().unwrap();
        let action = new_action("a1", ProviderId::Fake, "send_reply", "thread t1", None);
        store.create_action(&action).unwrap();

        store
            .transition_action("a1", ActionState::AwaitingApproval)
            .unwrap();
        store
            .transition_action("a1", ActionState::Executing)
            .unwrap();

        let illegal = store.transition_action("a1", ActionState::Planned);
        assert!(matches!(
            illegal,
            Err(StorageError::IllegalTransition { .. })
        ));
    }

    #[test]
    fn terminal_action_gets_completion_time() {
        let store = Store::open_in_memory().unwrap();
        let action = new_action("a2", ProviderId::Fake, "send_reply", "thread t1", None);
        store.create_action(&action).unwrap();
        store.transition_action("a2", ActionState::Failed).unwrap();

        let actions = store.recent_actions(10).unwrap();
        assert_eq!(actions[0].state, ActionState::Failed);
        assert!(actions[0].completed_at_ms.is_some());
    }

    #[test]
    fn audit_events_are_append_only() {
        let store = Store::open_in_memory().unwrap();
        store
            .append_audit(&AuditEvent {
                action_id: None,
                at_ms: now_ms(),
                status: "doctor".into(),
                detail: "ok".into(),
            })
            .unwrap();
        assert_eq!(store.audit_count().unwrap(), 1);
    }

    #[test]
    fn preferences_round_trip() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.get_preference("allow_send").unwrap(), None);
        store.set_preference("allow_send", "false").unwrap();
        assert_eq!(
            store.get_preference("allow_send").unwrap().as_deref(),
            Some("false")
        );
    }
}
