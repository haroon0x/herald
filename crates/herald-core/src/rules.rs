//! Deterministic command planning for the MVP.
//!
//! This is deliberately not an LLM. Common commands can be mapped without
//! inference, which is faster, testable, and cannot hallucinate an operation.
//! The local model is the fallback for phrasing these rules cannot cover.

use crate::action::{ActionPlan, Operation, Target};
use crate::provider::ProviderId;

/// Map a plain-language instruction to a plan without inference.
///
/// Returns `None` when the instruction needs the model, or when an outbound
/// body is missing. Callers should ask for clarification rather than guess.
#[must_use]
pub fn plan_offline(provider: ProviderId, instruction: &str) -> Option<ActionPlan> {
    let text = instruction.trim();
    if text.is_empty() {
        return None;
    }

    let lower = text.to_lowercase();
    let target = extract_target(text);

    let (operation, reply_body) = if starts_with_any(&lower, &["send", "reply", "respond"]) {
        (Operation::SendReply, extract_body(text)?)
    } else if lower.contains("draft") {
        (Operation::PrepareReply, extract_body(text)?)
    } else if starts_with_any(&lower, &["summarize", "summarise", "summary of"]) {
        (Operation::SummarizeThread, String::new())
    } else if starts_with_any(&lower, &["search", "find", "look for"]) {
        (Operation::SearchMessages, String::new())
    } else if lower.contains("unread")
        || lower.contains("anything important")
        || lower.contains("check")
        || lower.contains("inbox")
    {
        (Operation::ListUnreadMessages, String::new())
    } else if lower.contains("recent") || lower.contains("latest") || lower.contains("new") {
        (Operation::ListRecentMessages, String::new())
    } else {
        return None;
    };

    Some(ActionPlan {
        provider,
        operation,
        target,
        reply_body: (!reply_body.is_empty()).then_some(reply_body),
        needs_clarification: false,
        clarification_question: None,
    })
}

fn starts_with_any(lower: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|prefix| lower.starts_with(prefix))
}

fn extract_target(text: &str) -> Target {
    if let Some(index) = find_ascii_case_insensitive(text, " from ") {
        let sender = text[index + " from ".len()..].trim();
        let cut = [" about ", " re ", " regarding ", ",", "."]
            .iter()
            .filter_map(|marker| sender.find(marker))
            .min()
            .unwrap_or(sender.len());
        let sender = sender[..cut].trim();
        if !sender.is_empty() {
            return Target::LatestFrom {
                sender: sender.to_string(),
            };
        }
    }

    Target::Description {
        description: text.to_string(),
    }
}

fn find_ascii_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    let lower = haystack.to_lowercase();
    // Guard against case folding changing byte offsets.
    if lower.len() != haystack.len() {
        return haystack.find(needle);
    }
    lower.find(needle)
}

/// The unvalidated target a user's description points at.
///
/// Exposed as a hint for the tiered planner: the model (or CLI) produces a
/// human description, and the provider adapter resolves it against real data.
/// A hint is never a recipient.
#[must_use]
pub fn target_hint(text: &str) -> Target {
    extract_target(text)
}

/// The exact outbound text embedded in the instruction, if any.
///
/// Failure to find one is not an error: the caller must not invent wording.
#[must_use]
pub fn body_hint(text: &str) -> Option<String> {
    extract_body(text)
}

fn extract_body(text: &str) -> Option<String> {
    const MARKERS: [&str; 5] = [
        "saying",
        "that says",
        "with the text",
        "exactly:",
        "message:",
    ];

    let lower = text.to_lowercase();
    for marker in MARKERS {
        if let Some(index) = lower.find(marker) {
            let body = text[index + marker.len()..].trim();
            let body = body.trim_start_matches(':').trim().trim_matches('"').trim();
            if !body.is_empty() {
                return Some(body.to_string());
            }
        }
    }

    // Fall back to an explicit "draft/send: <body>" form.
    text.find(':').and_then(|index| {
        let body = text[index + 1..].trim().trim_matches('"').trim();
        (!body.is_empty()).then(|| body.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::validate_plan;

    fn plan(instruction: &str) -> ActionPlan {
        plan_offline(ProviderId::Gmail, instruction).expect("expected a deterministic plan")
    }

    #[test]
    fn check_mail_is_read_only() {
        let plan = plan("Check whether I have anything important.");
        assert_eq!(plan.operation, Operation::ListUnreadMessages);
        assert!(plan.reply_body.is_none());
    }

    #[test]
    fn summarize_extracts_sender() {
        let plan = plan("Summarize the latest email from the founder about the interview.");
        assert_eq!(plan.operation, Operation::SummarizeThread);
        assert_eq!(
            plan.target,
            Target::LatestFrom {
                sender: "the founder".into()
            }
        );
    }

    #[test]
    fn draft_keeps_requested_body() {
        let plan = plan("Draft a reply saying I'm interested, what are the next steps?");
        assert_eq!(plan.operation, Operation::PrepareReply);
        assert_eq!(
            plan.reply_body.as_deref(),
            Some("I'm interested, what are the next steps?")
        );
        assert!(validate_plan(plan).is_ok());
    }

    #[test]
    fn send_without_body_is_not_guessed() {
        assert!(plan_offline(ProviderId::Gmail, "Send this to Alex").is_none());
    }

    #[test]
    fn send_with_exact_body_is_preserved() {
        let plan = plan(r#"Send exactly: "Thanks, I'll review this.""#);
        assert_eq!(plan.operation, Operation::SendReply);
        assert_eq!(
            plan.reply_body.as_deref(),
            Some("Thanks, I'll review this.")
        );
    }

    #[test]
    fn unknown_instruction_needs_the_model() {
        assert!(plan_offline(ProviderId::Gmail, "Give me the gist of my day").is_none());
    }
}
