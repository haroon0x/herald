//! Errors planning and validation can produce. Everything here fails closed.

use crate::action::Operation;

/// Why a raw planner output cannot become an executable action.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    #[error("clarification required: {0}")]
    ClarificationRequired(String),
    #[error("the action target is missing or empty")]
    MissingTarget,
    #[error("operation {operation:?} requires a non-empty reply_body")]
    MissingReplyBody { operation: Operation },
    #[error("invalid plan: {0}")]
    InvalidPlan(String),
}
