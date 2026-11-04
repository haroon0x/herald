//! Authorization: who may do what. `authorize` turns a validated action into
//! `Allowed`, `NeedsApproval`, or `Rejected`. Sends are never `Allowed` except on the
//! autopost lane, which is scoped to original posts and every rail must pass.

use serde::{Deserialize, Serialize};

use crate::action::{Action, Target};
use crate::provider::ProviderId;
use crate::quality::passes_tweet_quality_floor;

/// Local policy knobs. In the MVP `allow_send` defaults to false: reading and
/// drafting work immediately, sending must be turned on deliberately.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyConfig {
    pub allow_send: bool,
    pub max_body_chars: usize,
    pub autopost: AutopostPolicy,
}

/// The ONLY path by which an outbound action can skip per-action approval —
/// and it exists solely for original X posts to the user's own timeline.
/// Every rail must pass, and the caller supplies the two computed rails
/// (`quiet_now`, `remaining_today`); the rest are configuration. Missing or
/// unknown values fail closed.
///
/// DMs and replies to mentions are never autoposted in this build. Widening
/// the surface is a policy decision that needs its own ADR and tests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutopostPolicy {
    /// Master switch. Off by default.
    pub enabled: bool,
    /// Maximum number of outbound posts per calendar day, across manual and
    /// autonomous sends.
    pub daily_limit: u32,
    /// Deterministic quality floor in `[0, 10]` (the Zyquor-calibrated scorer).
    pub min_quality: u8,
    /// True when the caller has determined the current time is inside the
    /// configured quiet hours.
    pub quiet_now: bool,
    /// Posts left in today's budget, computed by the caller from the action
    /// log. Zero blocks the lane.
    pub remaining_today: u32,
}

impl Default for AutopostPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            daily_limit: 5,
            min_quality: 7,
            quiet_now: false,
            remaining_today: 5,
        }
    }
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            allow_send: false,
            max_body_chars: 8_000,
            autopost: AutopostPolicy::default(),
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
/// Reads and drafts are local and need no approval. A send to the timeline
/// (`create_post`) may run without an approval when EVERY autopost rail
/// passes; anything else — any rail failing, or a target that is not the
/// timeline — falls back to the approval queue. Nothing is ever sent
/// silently.
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

            if autopost_allows(send, &config.autopost) {
                return Decision::Allowed;
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

/// Every rail must pass. Anything else — disabled switch, quiet hours, an
/// exhausted daily budget, a body below the floor — fails closed.
fn autopost_allows(send: &crate::action::SendAction, autopost: &AutopostPolicy) -> bool {
    if !autopost.enabled || autopost.quiet_now || autopost.remaining_today == 0 {
        return false;
    }
    if !matches!(send.target, Target::Timeline) {
        return false;
    }
    passes_tweet_quality_floor(&send.body, autopost.min_quality)
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
            ..PolicyConfig::default()
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
            ..PolicyConfig::default()
        };
        assert!(matches!(
            authorize(&send("hello"), &policy),
            Decision::Rejected(Rejection::BodyTooLong { .. })
        ));
    }

    fn timeline_send(body: &str) -> Action {
        Action::Send(SendAction {
            provider: ProviderId::X,
            target: Target::Timeline,
            body: body.to_string(),
            body_hash: body_hash(body),
        })
    }

    fn autopost_policy() -> PolicyConfig {
        PolicyConfig {
            allow_send: true,
            autopost: AutopostPolicy {
                enabled: true,
                daily_limit: 3,
                min_quality: 0, // quality rail disabled for mechanics tests
                quiet_now: false,
                remaining_today: 3,
            },
            ..PolicyConfig::default()
        }
    }

    #[test]
    fn timeline_post_does_not_autopost_by_default() {
        let policy = PolicyConfig {
            allow_send: true,
            ..PolicyConfig::default()
        };
        assert!(matches!(
            authorize(&timeline_send("hello world"), &policy),
            Decision::NeedsApproval(_)
        ));
    }

    #[test]
    fn timeline_post_autoposts_when_every_rail_passes() {
        assert!(matches!(
            authorize(&timeline_send("Thursday works."), &autopost_policy()),
            Decision::Allowed
        ));
    }

    #[test]
    fn thread_send_never_autoposts() {
        let policy = autopost_policy();
        assert!(matches!(
            authorize(&send("hi"), &policy),
            Decision::NeedsApproval(_)
        ));
    }

    #[test]
    fn quiet_hours_block_autopost() {
        let mut policy = autopost_policy();
        policy.autopost.quiet_now = true;
        assert!(matches!(
            authorize(&timeline_send("Thursday works."), &policy),
            Decision::NeedsApproval(_)
        ));
    }

    #[test]
    fn exhausted_budget_blocks_autopost() {
        let mut policy = autopost_policy();
        policy.autopost.remaining_today = 0;
        assert!(matches!(
            authorize(&timeline_send("Thursday works."), &policy),
            Decision::NeedsApproval(_)
        ));
    }

    #[test]
    fn low_quality_blocks_autopost() {
        let mut policy = autopost_policy();
        policy.autopost.min_quality = 9; // "Thursday works." scores 3
        assert!(matches!(
            authorize(&timeline_send("Thursday works."), &policy),
            Decision::NeedsApproval(_)
        ));
    }

    #[test]
    fn slop_content_blocks_autopost() {
        let policy = autopost_policy();
        assert!(matches!(
            authorize(
                &timeline_send("let's dive in!!! game-changer, leverage unlock synergy"),
                &policy
            ),
            Decision::NeedsApproval(_)
        ));
    }
}
