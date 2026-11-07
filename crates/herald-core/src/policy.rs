//! Authorization: who may do what. `authorize` turns a validated action into
//! `Allowed`, `NeedsApproval`, or `Rejected`. No outbound action is ever
//! `Allowed`; every send stops at a durable, human-verifiable approval.

use serde::{Deserialize, Serialize};

use crate::action::{Action, Target};
use crate::provider::ProviderId;

/// Local policy knobs. `allow_send` defaults to false: reading and drafting
/// work immediately, sending must be turned on deliberately.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyConfig {
    pub allow_send: bool,
    pub max_body_chars: usize,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            allow_send: false,
            max_body_chars: 8_000,
        }
    }
}

/// The exact thing the user is approving. Every field is displayed before an
/// outbound action executes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub provider: ProviderId,
    pub target: Target,
    pub body: String,
    pub body_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rejection {
    /// Outbound actions are disabled by configuration.
    SendDisabled,
    BodyTooLong {
        chars: usize,
        max: usize,
    },
}

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SendDisabled => f.write_str("sending is disabled by policy"),
            Self::BodyTooLong { chars, max } => {
                write!(f, "reply body is {chars} characters, limit is {max}")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allowed,
    NeedsApproval(ApprovalRequest),
    Rejected(Rejection),
}

impl Decision {
    #[must_use]
    pub const fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed)
    }

    #[must_use]
    pub const fn requires_approval(&self) -> bool {
        matches!(self, Self::NeedsApproval(_))
    }
}

/// Apply the approval policy to a validated action.
///
/// Reads and drafts are local and need no approval. Every send queues for a
/// durable, human-verifiable approval — nothing is ever sent silently.
#[must_use]
pub fn authorize(action: &Action, config: &PolicyConfig) -> Decision {
    match action {
        Action::Read(_) | Action::Draft(_) => Decision::Allowed,
        Action::Send(send) => {
            if !config.allow_send {
                return Decision::Rejected(Rejection::SendDisabled);
            }

            let chars = send.body.chars().count();
            if chars > config.max_body_chars {
                return Decision::Rejected(Rejection::BodyTooLong {
                    chars,
                    max: config.max_body_chars,
                });
            }

            Decision::NeedsApproval(ApprovalRequest {
                provider: send.provider,
                target: send.target.clone(),
                body: send.body.clone(),
                body_hash: send.body_hash.clone(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::action::body_hash;
    use crate::action::{Action, DraftAction, ReadAction, ReadOperation, SendAction, Target};
    use crate::provider::ProviderId;

    use super::*;

    fn read() -> Action {
        Action::Read(ReadAction {
            provider: ProviderId::Gmail,
            operation: ReadOperation::ListUnreadMessages,
            target: Target::Description {
                description: "unread".into(),
            },
        })
    }

    fn send(body: &str) -> Action {
        Action::Send(SendAction {
            provider: ProviderId::Gmail,
            target: Target::Thread {
                thread_id: crate::action::ThreadId::new("t1"),
            },
            body: body.to_string(),
            body_hash: body_hash(body),
        })
    }

    #[test]
    fn reads_need_no_approval() {
        assert!(authorize(&read(), &PolicyConfig::default()).is_allowed());
    }

    #[test]
    fn drafts_need_no_approval() {
        let action = Action::Draft(DraftAction {
            provider: ProviderId::Gmail,
            target: Target::Thread {
                thread_id: crate::action::ThreadId::new("t1"),
            },
            body: "draft".into(),
        });
        assert!(authorize(&action, &PolicyConfig::default()).is_allowed());
    }

    #[test]
    fn sending_is_disabled_by_default() {
        assert_eq!(
            authorize(&send("hi"), &PolicyConfig::default()),
            Decision::Rejected(Rejection::SendDisabled)
        );
    }

    #[test]
    fn sending_requires_approval_when_enabled() {
        let policy = PolicyConfig {
            allow_send: true,
            max_body_chars: 8_000,
        };
        match authorize(&send("hi"), &policy) {
            Decision::NeedsApproval(request) => {
                assert_eq!(request.body, "hi");
                assert_eq!(request.body_hash, body_hash("hi"));
            }
            other => panic!("expected approval, got {other:?}"),
        }
    }

    #[test]
    fn overlong_body_is_rejected() {
        let policy = PolicyConfig {
            allow_send: true,
            max_body_chars: 4,
        };
        assert!(matches!(
            authorize(&send("hello"), &policy),
            Decision::Rejected(Rejection::BodyTooLong { .. })
        ));
    }

    #[test]
    fn timeline_send_needs_approval_and_never_auto_sends() {
        // The timeline target survives only for legacy queue entries. A send
        // aimed at it is indistinguishable from any other send here: it can
        // never be allowed, whatever the configuration.
        let policy = PolicyConfig {
            allow_send: true,
            max_body_chars: 8_000,
        };
        let legacy = Action::Send(SendAction {
            provider: ProviderId::X,
            target: Target::Timeline,
            body: "hello world".into(),
            body_hash: body_hash("hello world"),
        });
        assert!(matches!(
            authorize(&legacy, &policy),
            Decision::NeedsApproval(_)
        ));
    }
}
