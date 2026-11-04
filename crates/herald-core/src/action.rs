//! The typed action ladder's rungs 1-2: `ActionPlan` (untrusted) to `Action` (validated).
//!
//! `validate_plan` is the only gate between model output and providers. It rejects
//! ambiguity, missing bodies, and unknown targets; it never rewrites a body and never
//! invents an identifier.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

use crate::error::PlanError;
use crate::provider::ProviderId;

/// Stable thread identifier from a provider.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ThreadId(pub String);

impl ThreadId {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ThreadId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// What the user wants done. Only these operations exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    ListRecentMessages,
    ListUnreadMessages,
    GetThread,
    SearchMessages,
    SummarizeThread,
    PrepareReply,
    SendReply,
    CreatePost,
}

impl Operation {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ListRecentMessages => "list_recent_messages",
            Self::ListUnreadMessages => "list_unread_messages",
            Self::GetThread => "get_thread",
            Self::SearchMessages => "search_messages",
            Self::SummarizeThread => "summarize_thread",
            Self::PrepareReply => "prepare_reply",
            Self::SendReply => "send_reply",
            Self::CreatePost => "create_post",
        }
    }

    #[must_use]
    pub const fn is_outbound(self) -> bool {
        matches!(self, Self::SendReply | Self::CreatePost)
    }

    #[must_use]
    pub const fn needs_body(self) -> bool {
        matches!(
            self,
            Self::PrepareReply | Self::SendReply | Self::CreatePost
        )
    }
}

/// The unvalidated target the model asked for.
///
/// A description is not an identifier. Rust must resolve it against real
/// provider data before anything is executed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    /// Human description, e.g. "latest email from the founder about the interview".
    Description { description: String },
    /// A thread id supplied by the app or a prior tool result, never guessed.
    Thread { thread_id: ThreadId },
    /// The latest message from `sender`, resolved by Rust.
    LatestFrom { sender: String },
    /// The user's own X timeline. Valid only for `create_post`; it is not a
    /// conversation and it never identifies a recipient.
    Timeline,
}

impl Target {
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Description { description } => description.clone(),
            Self::Thread { thread_id } => format!("thread {thread_id}"),
            Self::LatestFrom { sender } => format!("latest message from {sender}"),
            Self::Timeline => "your timeline".to_string(),
        }
    }
}

/// Raw planner output. Deserialized with `deny_unknown_fields`, so a model that
/// invents a field fails closed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionPlan {
    pub provider: ProviderId,
    pub operation: Operation,
    pub target: Target,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_body: Option<String>,
    #[serde(default)]
    pub needs_clarification: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clarification_question: Option<String>,
}

/// Read-only action after validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadAction {
    pub provider: ProviderId,
    pub operation: ReadOperation,
    pub target: Target,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadOperation {
    ListRecentMessages,
    ListUnreadMessages,
    GetThread,
    SearchMessages,
    SummarizeThread,
}

/// A draft is local until the user sends it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftAction {
    pub provider: ProviderId,
    pub target: Target,
    pub body: String,
}

/// An approved-to-be-considered send. Policy still decides whether it executes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendAction {
    pub provider: ProviderId,
    pub target: Target,
    pub body: String,
    pub body_hash: String,
}

/// Fully validated action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Read(ReadAction),
    Draft(DraftAction),
    Send(SendAction),
}

impl Action {
    #[must_use]
    pub const fn provider(&self) -> ProviderId {
        match self {
            Self::Read(a) => a.provider,
            Self::Draft(a) => a.provider,
            Self::Send(a) => a.provider,
        }
    }

    #[must_use]
    pub fn target(&self) -> &Target {
        match self {
            Self::Read(a) => &a.target,
            Self::Draft(a) => &a.target,
            Self::Send(a) => &a.target,
        }
    }
}

/// Validate a raw plan into a typed action.
///
/// This is the only gate between model output and provider code. It rejects
/// ambiguity, missing bodies, and operations that do not exist.
pub fn validate_plan(plan: ActionPlan) -> Result<Action, PlanError> {
    if plan.needs_clarification {
        return Err(PlanError::ClarificationRequired(
            plan.clarification_question
                .filter(|q| !q.trim().is_empty())
                .unwrap_or_else(|| "Which message do you mean?".to_string()),
        ));
    }

    validate_target(&plan.target)?;

    match plan.operation {
        Operation::PrepareReply => {
            let body = require_body(plan.reply_body, plan.operation)?;
            Ok(Action::Draft(DraftAction {
                provider: plan.provider,
                target: plan.target,
                body,
            }))
        }
        Operation::SendReply | Operation::CreatePost => {
            let body = require_body(plan.reply_body, plan.operation)?;
            let body_hash = body_hash(&body);
            Ok(Action::Send(SendAction {
                provider: plan.provider,
                target: plan.target,
                body,
                body_hash,
            }))
        }
        read => Ok(Action::Read(ReadAction {
            provider: plan.provider,
            operation: read_operation(read)?,
            target: plan.target,
        })),
    }
}

fn read_operation(operation: Operation) -> Result<ReadOperation, PlanError> {
    match operation {
        Operation::ListRecentMessages => Ok(ReadOperation::ListRecentMessages),
        Operation::ListUnreadMessages => Ok(ReadOperation::ListUnreadMessages),
        Operation::GetThread => Ok(ReadOperation::GetThread),
        Operation::SearchMessages => Ok(ReadOperation::SearchMessages),
        Operation::SummarizeThread => Ok(ReadOperation::SummarizeThread),
        Operation::PrepareReply | Operation::SendReply | Operation::CreatePost => Err(
            PlanError::InvalidPlan("reply operation is not a read action".to_string()),
        ),
    }
}

fn validate_target(target: &Target) -> Result<(), PlanError> {
    match target {
        Target::Description { description } if description.trim().is_empty() => {
            Err(PlanError::MissingTarget)
        }
        Target::LatestFrom { sender } if sender.trim().is_empty() => Err(PlanError::MissingTarget),
        Target::Thread { thread_id } if thread_id.as_str().trim().is_empty() => {
            Err(PlanError::MissingTarget)
        }
        _ => Ok(()),
    }
}

fn require_body(body: Option<String>, operation: Operation) -> Result<String, PlanError> {
    let Some(body) = body else {
        return Err(PlanError::MissingReplyBody { operation });
    };

    if body.trim().is_empty() {
        return Err(PlanError::MissingReplyBody { operation });
    }

    Ok(body)
}

/// SHA-256 of the exact outbound text. Used to detect edits between approval and
/// execution and as the idempotency content hash.
#[must_use]
pub fn body_hash(body: &str) -> String {
    let digest = Sha256::digest(body.as_bytes());
    format!("{digest:x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> ActionPlan {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn valid_send_plan_becomes_typed_action() {
        let plan = parse(
            r#"{
                "provider": "gmail",
                "operation": "send_reply",
                "target": {"kind": "thread", "thread_id": "t1"},
                "reply_body": "Thanks, I'll review this."
            }"#,
        );
        let action = validate_plan(plan).unwrap();
        match action {
            Action::Send(send) => {
                assert_eq!(send.body, "Thanks, I'll review this.");
                assert_eq!(send.body_hash, body_hash("Thanks, I'll review this."));
            }
            other => panic!("expected send, got {other:?}"),
        }
    }

    #[test]
    fn send_without_body_is_rejected() {
        let plan = parse(
            r#"{
                "provider": "gmail",
                "operation": "send_reply",
                "target": {"kind": "thread", "thread_id": "t1"}
            }"#,
        );
        assert!(matches!(
            validate_plan(plan),
            Err(PlanError::MissingReplyBody { .. })
        ));
    }

    #[test]
    fn whitespace_body_is_rejected() {
        let plan = parse(
            r#"{
                "provider": "gmail",
                "operation": "prepare_reply",
                "target": {"kind": "thread", "thread_id": "t1"},
                "reply_body": "   "
            }"#,
        );
        assert!(matches!(
            validate_plan(plan),
            Err(PlanError::MissingReplyBody { .. })
        ));
    }

    #[test]
    fn unknown_field_fails_closed() {
        let result = serde_json::from_str::<ActionPlan>(
            r#"{
                "provider": "gmail",
                "operation": "send_reply",
                "target": {"kind": "thread", "thread_id": "t1"},
                "reply_body": "hi",
                "run_javascript": "steal()"
            }"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn unknown_provider_fails_closed() {
        let result = serde_json::from_str::<ActionPlan>(
            r#"{
                "provider": "outlook",
                "operation": "list_unread_messages",
                "target": {"kind": "description", "description": "unread"}
            }"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn clarification_plan_never_executes() {
        let plan = parse(
            r#"{
                "provider": "gmail",
                "operation": "summarize_thread",
                "target": {"kind": "description", "description": "from Sam"},
                "needs_clarification": true,
                "clarification_question": "Which Sam?"
            }"#,
        );
        match validate_plan(plan) {
            Err(PlanError::ClarificationRequired(question)) => assert_eq!(question, "Which Sam?"),
            other => panic!("expected clarification, got {other:?}"),
        }
    }

    #[test]
    fn body_is_never_rewritten() {
        let exact = "  Thanks, I'll review this.  ";
        let plan = parse(&format!(
            r#"{{
                "provider": "gmail",
                "operation": "send_reply",
                "target": {{"kind": "thread", "thread_id": "t1"}},
                "reply_body": {exact:?}
            }}"#
        ));
        match validate_plan(plan).unwrap() {
            Action::Send(send) => assert_eq!(send.body, exact),
            other => panic!("expected send, got {other:?}"),
        }
    }
}
