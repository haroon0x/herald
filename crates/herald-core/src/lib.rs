//! Core domain for Herald.
//!
//! Everything in this crate is provider-agnostic and side-effect free. The
//! model produces [`ActionPlan`] values; this crate turns them into validated
//! [`Action`] values, applies the approval [`PolicyConfig`], and tracks the
//! idempotent [`ActionState`] machine.

pub mod action;
pub mod diff;
pub mod error;
pub mod intent;
pub mod policy;
pub mod provider;
pub mod rules;
pub mod state;
pub mod time;

pub use action::{
    Action, ActionPlan, DraftAction, Operation, ReadAction, ReadOperation, SendAction, Target,
    ThreadId, body_hash, canonical_body_text, validate_plan, validate_target,
};
pub use diff::{DiffKind, DiffSegment, word_diff};
pub use error::PlanError;
pub use policy::{ApprovalRequest, Decision, PolicyConfig, Rejection, authorize};
pub use provider::{
    MessageId, MessageQuery, MessageScope, MessageSummary, ProviderId, ProviderMessage, Thread,
    Untrusted,
};
pub use state::{ActionState, TransitionError};
