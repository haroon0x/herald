use serde::{Deserialize, Serialize};

/// Lifecycle of a write action.
///
/// `Uncertain` is the important state: an SMTP or provider timeout may mean the
/// message was sent. Reconciliation must resolve it before any retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionState {
    Planned,
    AwaitingApproval,
    Executing,
    Succeeded,
    Failed,
    Uncertain,
    Cancelled,
}

impl ActionState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::AwaitingApproval => "awaiting_approval",
            Self::Executing => "executing",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Uncertain => "uncertain",
            Self::Cancelled => "cancelled",
        }
    }

    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }

    /// Returns the next state or an error for an illegal transition.
    pub fn transition(self, next: Self) -> Result<Self, TransitionError> {
        let allowed = matches!(
            (self, next),
            (
                Self::Planned,
                Self::AwaitingApproval | Self::Executing | Self::Failed | Self::Cancelled
            ) | (
                Self::AwaitingApproval,
                Self::Executing | Self::Failed | Self::Cancelled
            ) | (
                Self::Executing,
                Self::Succeeded | Self::Failed | Self::Uncertain
            ) | (Self::Uncertain, Self::Succeeded | Self::Failed)
        );

        if allowed {
            Ok(next)
        } else {
            Err(TransitionError {
                from: self,
                to: next,
            })
        }
    }
}

impl std::str::FromStr for ActionState {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "planned" => Ok(Self::Planned),
            "awaiting_approval" => Ok(Self::AwaitingApproval),
            "executing" => Ok(Self::Executing),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "uncertain" => Ok(Self::Uncertain),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(format!("unknown action state: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("illegal action transition: {from:?} -> {to:?}")]
pub struct TransitionError {
    pub from: ActionState,
    pub to: ActionState,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn happy_path_transitions() {
        let state = ActionState::Planned
            .transition(ActionState::AwaitingApproval)
            .unwrap()
            .transition(ActionState::Executing)
            .unwrap()
            .transition(ActionState::Succeeded)
            .unwrap();
        assert_eq!(state, ActionState::Succeeded);
        assert!(state.is_terminal());
    }

    #[test]
    fn uncertain_can_be_reconciled() {
        let state = ActionState::Executing
            .transition(ActionState::Uncertain)
            .unwrap();
        assert_eq!(
            state.transition(ActionState::Succeeded).unwrap(),
            ActionState::Succeeded
        );
        assert_eq!(
            state.transition(ActionState::Failed).unwrap(),
            ActionState::Failed
        );
    }

    #[test]
    fn terminal_states_cannot_move() {
        assert!(
            ActionState::Succeeded
                .transition(ActionState::Executing)
                .is_err()
        );
        assert!(
            ActionState::Failed
                .transition(ActionState::Executing)
                .is_err()
        );
    }

    #[test]
    fn cannot_execute_without_planning() {
        assert!(
            ActionState::AwaitingApproval
                .transition(ActionState::Succeeded)
                .is_err()
        );
    }

    #[test]
    fn a_pending_approval_can_be_cancelled() {
        let state = ActionState::AwaitingApproval
            .transition(ActionState::Cancelled)
            .unwrap();
        assert_eq!(state, ActionState::Cancelled);
        assert!(state.is_terminal());
    }

    #[test]
    fn cancelled_is_terminal_and_cannot_resume() {
        assert!(
            ActionState::Cancelled
                .transition(ActionState::Executing)
                .is_err()
        );
        assert!(
            ActionState::Cancelled
                .transition(ActionState::Succeeded)
                .is_err()
        );
    }

    #[test]
    fn executing_cannot_be_cancelled_silently() {
        // Once a send may have reached the provider, the only legal outcomes
        // are success, failure, or `uncertain` followed by reconciliation.
        assert!(
            ActionState::Executing
                .transition(ActionState::Cancelled)
                .is_err()
        );
    }
}
