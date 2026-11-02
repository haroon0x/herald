//! Core domain for Herald.
//!
//! Everything in this crate is provider-agnostic and side-effect free. The
//! model produces [`ActionPlan`] values; this crate turns them into validated
//! [`Action`] values, applies the approval [`PolicyConfig`], and tracks the
//! idempotent [`ActionState`] machine.

pub mod action;
pub mod error;
pub mod policy;
pub mod prompt;
pub mod provider;
pub mod rules;
pub mod state;
pub mod time;

pub use action::{
    Action, ActionPlan, DraftAction, Operation, ReadAction, ReadOperation, SendAction, Target,
    ThreadId, body_hash, validate_plan,
};
pub use error::PlanError;
pub use policy::{ApprovalRequest, Decision, PolicyConfig, Rejection, authorize};
pub use provider::{
    MessageId, MessageQuery, MessageSummary, ProviderId, ProviderMessage, Thread, Untrusted,
};
pub use state::{ActionState, TransitionError};
