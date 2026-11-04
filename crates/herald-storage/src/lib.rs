//! `SQLite` state and audit log.
//!
//! The store is the source of truth for which messages have been seen and what
//! happened to every write action. The model never owns this state.

use std::path::Path;
use std::str::FromStr;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use herald_core::{MessageId, MessageSummary, ProviderId, Target, ThreadId, time::now_ms};

pub use herald_core::ActionState;

const SCHEMA: &str = include_str!("../schema.sql");
const SCHEMA_VERSION: i64 = 3;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unknown action state in database: {0}")]
    BadState(String),
    #[error("action {0} was not found")]
    ActionNotFound(String),
    #[error("an approval already exists for action {0}")]
    ApprovalExists(String),
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

impl StoredMessage {
    /// Attribute a summary to the provider that produced it.
    ///
    /// The provider is always passed in. A summary carries no provider of its
    /// own, and guessing one would corrupt dedupe across accounts.
    #[must_use]
    pub fn from_summary(provider: ProviderId, summary: &MessageSummary) -> Self {
        Self {
            provider,
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
    /// Human-readable target, for display and audit.
    pub target: String,
    /// The original planner target as JSON, so it can be re-resolved later.
    pub target_json: Option<String>,
    /// The thread the target resolved to, if resolution happened.
    pub thread_id: Option<ThreadId>,
    pub content_hash: Option<String>,
    pub state: ActionState,
    pub created_at_ms: i64,
    pub completed_at_ms: Option<i64>,
}

/// An outbound action waiting for a human decision.
///
/// The body lives here only until the decision (or expiry), because the
/// approval card must show the exact bytes that will be sent. It is never
/// copied into the audit log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingApproval {
    pub action_id: String,
    pub provider: ProviderId,
    /// The original planner target, re-resolved before sending.
    pub target: Target,
    /// The thread the target resolved to at plan time. Approve re-resolves and
    /// fails closed if this changed.
    pub thread_id: ThreadId,
    pub body: String,
    pub body_hash: String,
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
        let path = path.as_ref();
        let conn = Connection::open(path)?;
        restrict_permissions(path)?;
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
            // Older databases created `actions` before the resolution columns
            // existed. `CREATE TABLE IF NOT EXISTS` cannot add them.
            ensure_column(&conn, "actions", "thread_id", "thread_id TEXT")?;
            ensure_column(&conn, "actions", "target_json", "target_json TEXT")?;
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
                (action_id, provider, operation, target, target_json, thread_id,
                 content_hash, state, created_at, completed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                action.action_id,
                action.provider.as_str(),
                action.operation,
                action.target,
                action.target_json,
                action.thread_id.as_ref().map(ThreadId::as_str),
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
            "SELECT action_id, provider, operation, target, target_json, thread_id,
                    content_hash, state, created_at, completed_at
             FROM actions ORDER BY created_at DESC LIMIT ?1",
        )?;

        let rows = statement.query_map(
            params![i64::try_from(limit).unwrap_or(i64::MAX)],
            read_action_row,
        )?;

        let mut actions = Vec::new();
        for row in rows {
            actions.push(action_from_row(row?)?);
        }
        Ok(actions)
    }

    pub fn get_action(&self, action_id: &str) -> Result<Option<ActionRecord>, StorageError> {
        let row = self
            .conn
            .query_row(
                "SELECT action_id, provider, operation, target, target_json, thread_id,
                        content_hash, state, created_at, completed_at
                 FROM actions WHERE action_id = ?1",
                params![action_id],
                read_action_row,
            )
            .optional()?;

        row.map(action_from_row).transpose()
    }

    /// Persist an approval request. The body is retained only until the
    /// decision or expiry.
    pub fn create_approval(&self, approval: &PendingApproval) -> Result<(), StorageError> {
        let target = serde_json::to_string(&approval.target)?;
        let inserted = self.conn.execute(
            "INSERT INTO pending_approvals
                (action_id, provider, target, thread_id, body, body_hash, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(action_id) DO NOTHING",
            params![
                approval.action_id,
                approval.provider.as_str(),
                target,
                approval.thread_id.as_str(),
                approval.body,
                approval.body_hash,
                approval.created_at_ms,
                approval.expires_at_ms,
            ],
        )?;

        if inserted == 0 {
            return Err(StorageError::ApprovalExists(approval.action_id.clone()));
        }
        Ok(())
    }

    pub fn get_approval(&self, action_id: &str) -> Result<Option<PendingApproval>, StorageError> {
        let row = self
            .conn
            .query_row(
                "SELECT action_id, provider, target, thread_id, body, body_hash, created_at, expires_at
                 FROM pending_approvals WHERE action_id = ?1",
                params![action_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, i64>(7)?,
                    ))
                },
            )
            .optional()?;

        row.map(approval_from_row).transpose()
    }

    pub fn pending_approvals(&self) -> Result<Vec<PendingApproval>, StorageError> {
        let mut statement = self.conn.prepare(
            "SELECT action_id, provider, target, thread_id, body, body_hash, created_at, expires_at
             FROM pending_approvals ORDER BY created_at ASC",
        )?;

        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
            ))
        })?;

        let mut approvals = Vec::new();
        for row in rows {
            approvals.push(approval_from_row(row?)?);
        }
        Ok(approvals)
    }

    /// Delete the retained body after a decision. Returns whether a row existed.
    pub fn delete_approval(&self, action_id: &str) -> Result<bool, StorageError> {
        let deleted = self.conn.execute(
            "DELETE FROM pending_approvals WHERE action_id = ?1",
            params![action_id],
        )?;
        Ok(deleted == 1)
    }

    /// Drop approvals past their deadline. Returns how many bodies were deleted.
    pub fn purge_expired_approvals(&self, now_ms: i64) -> Result<usize, StorageError> {
        let deleted = self.conn.execute(
            "DELETE FROM pending_approvals WHERE expires_at <= ?1",
            params![now_ms],
        )?;
        Ok(deleted)
    }

    pub fn approval_count(&self) -> Result<usize, StorageError> {
        let count: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM pending_approvals", [], |row| {
                    row.get(0)
                })?;
        Ok(usize::try_from(count).unwrap_or(usize::MAX))
    }

    /// Cancel a pending action and drop its retained body.
    pub fn cancel_action(&self, action_id: &str) -> Result<ActionState, StorageError> {
        let state = self.transition_action(action_id, ActionState::Cancelled)?;
        self.delete_approval(action_id)?;
        Ok(state)
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

    pub fn recent_audit(&self, limit: usize) -> Result<Vec<AuditEvent>, StorageError> {
        let mut statement = self.conn.prepare(
            "SELECT action_id, at_ms, status, detail
             FROM audit_events ORDER BY id DESC LIMIT ?1",
        )?;
        let rows =
            statement.query_map(params![i64::try_from(limit).unwrap_or(i64::MAX)], |row| {
                Ok(AuditEvent {
                    action_id: row.get(0)?,
                    at_ms: row.get(1)?,
                    status: row.get(2)?,
                    detail: row.get(3)?,
                })
            })?;

        let mut events = Vec::new();
        for event in rows {
            events.push(event?);
        }
        Ok(events)
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
        target_json: None,
        thread_id: None,
        content_hash,
        state: ActionState::Planned,
        created_at_ms: now_ms(),
        completed_at_ms: None,
    }
}

type ActionRow = (
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    i64,
    Option<i64>,
);

fn read_action_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ActionRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
    ))
}

fn action_from_row(row: ActionRow) -> Result<ActionRecord, StorageError> {
    let (
        action_id,
        provider,
        operation,
        target,
        target_json,
        thread_id,
        content_hash,
        state,
        created_at,
        completed,
    ) = row;
    Ok(ActionRecord {
        action_id,
        provider: ProviderId::from_str(&provider).map_err(StorageError::BadState)?,
        operation,
        target,
        target_json,
        thread_id: thread_id.map(ThreadId::new),
        content_hash,
        state: ActionState::from_str(&state).map_err(StorageError::BadState)?,
        created_at_ms: created_at,
        completed_at_ms: completed,
    })
}

type ApprovalRow = (String, String, String, String, String, String, i64, i64);

fn approval_from_row(row: ApprovalRow) -> Result<PendingApproval, StorageError> {
    let (action_id, provider, target, thread_id, body, body_hash, created_at, expires_at) = row;
    Ok(PendingApproval {
        action_id,
        provider: ProviderId::from_str(&provider).map_err(StorageError::BadState)?,
        target: serde_json::from_str(&target)?,
        thread_id: ThreadId::new(thread_id),
        body,
        body_hash,
        created_at_ms: created_at,
        expires_at_ms: expires_at,
    })
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) -> Result<(), StorageError> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(0o600);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> Result<(), StorageError> {
    Ok(())
}

/// Add a column to an existing table when a migration needs it.
///
/// Table and column names are internal constants, never user input.
fn ensure_column(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<(), StorageError> {
    let mut statement = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;

    if !names.iter().any(|name| name == column) {
        conn.execute(&format!("ALTER TABLE {table} ADD COLUMN {definition}"), [])?;
    }
    Ok(())
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

    fn approval(action_id: &str, expires_at_ms: i64) -> PendingApproval {
        PendingApproval {
            action_id: action_id.into(),
            provider: ProviderId::Maildir,
            target: Target::Description {
                description: "latest message from Sam".into(),
            },
            thread_id: ThreadId::new("t-sam"),
            body: "Thanks, Thursday works.".into(),
            body_hash: herald_core::body_hash("Thanks, Thursday works."),
            created_at_ms: now_ms(),
            expires_at_ms,
        }
    }

    fn pending_action(store: &Store, action_id: &str) {
        store
            .create_action(&new_action(
                action_id,
                ProviderId::Maildir,
                "send_reply",
                "latest message from Sam",
                Some(herald_core::body_hash("Thanks, Thursday works.")),
            ))
            .unwrap();
        store
            .transition_action(action_id, ActionState::AwaitingApproval)
            .unwrap();
    }

    #[test]
    fn approvals_round_trip_with_the_original_target() {
        let store = Store::open_in_memory().unwrap();
        pending_action(&store, "a1");
        store
            .create_approval(&approval("a1", now_ms() + 60_000))
            .unwrap();

        let stored = store.get_approval("a1").unwrap().unwrap();
        assert_eq!(stored.provider, ProviderId::Maildir);
        assert_eq!(stored.thread_id, ThreadId::new("t-sam"));
        assert_eq!(
            stored.target,
            Target::Description {
                description: "latest message from Sam".into()
            }
        );
        assert_eq!(stored.body, "Thanks, Thursday works.");
        assert_eq!(store.approval_count().unwrap(), 1);
    }

    #[test]
    fn an_action_cannot_have_two_pending_approvals() {
        let store = Store::open_in_memory().unwrap();
        pending_action(&store, "a1");
        store
            .create_approval(&approval("a1", now_ms() + 60_000))
            .unwrap();
        let second = store.create_approval(&approval("a1", now_ms() + 120_000));
        assert!(matches!(second, Err(StorageError::ApprovalExists(_))));
        assert_eq!(store.approval_count().unwrap(), 1);
    }

    #[test]
    fn expired_approvals_are_purged_and_bodies_disappear() {
        let store = Store::open_in_memory().unwrap();
        pending_action(&store, "old");
        pending_action(&store, "fresh");
        store.create_approval(&approval("old", 10)).unwrap();
        store
            .create_approval(&approval("fresh", now_ms() + 60_000))
            .unwrap();

        assert_eq!(store.purge_expired_approvals(now_ms()).unwrap(), 1);
        assert!(store.get_approval("old").unwrap().is_none());
        assert!(store.get_approval("fresh").unwrap().is_some());
    }

    #[test]
    fn cancelling_an_action_drops_the_retained_body() {
        let store = Store::open_in_memory().unwrap();
        pending_action(&store, "a1");
        store
            .create_approval(&approval("a1", now_ms() + 60_000))
            .unwrap();

        assert_eq!(store.cancel_action("a1").unwrap(), ActionState::Cancelled);
        assert!(store.get_approval("a1").unwrap().is_none());
        assert_eq!(
            store.get_action("a1").unwrap().unwrap().state,
            ActionState::Cancelled
        );
    }

    #[test]
    fn deleting_an_approval_is_idempotent() {
        let store = Store::open_in_memory().unwrap();
        pending_action(&store, "a1");
        store
            .create_approval(&approval("a1", now_ms() + 60_000))
            .unwrap();
        assert!(store.delete_approval("a1").unwrap());
        assert!(!store.delete_approval("a1").unwrap());
    }

    #[test]
    fn get_action_returns_none_for_unknown_ids() {
        let store = Store::open_in_memory().unwrap();
        assert!(store.get_action("nope").unwrap().is_none());
    }

    #[test]
    fn audit_events_read_back_newest_first() {
        let store = Store::open_in_memory().unwrap();
        for (at_ms, status) in [(1, "first"), (2, "second")] {
            store
                .append_audit(&AuditEvent {
                    action_id: None,
                    at_ms,
                    status: status.into(),
                    detail: String::new(),
                })
                .unwrap();
        }
        let events = store.recent_audit(10).unwrap();
        assert_eq!(events[0].status, "second");
        assert_eq!(events[1].status, "first");
    }

    #[test]
    fn summaries_keep_the_provider_that_produced_them() {
        let summary = MessageSummary {
            id: MessageId::new("m1"),
            thread_id: ThreadId::new("t1"),
            sender: "Sam".into(),
            subject: "Lunch".into(),
            timestamp_ms: 1,
            unread: true,
        };
        let stored = StoredMessage::from_summary(ProviderId::Maildir, &summary);
        assert_eq!(stored.provider, ProviderId::Maildir);
    }
}
