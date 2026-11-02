//! Tiered planner prompts.
//!
//! Design goal from the Phase 0 spike: never ask the local model to hold intent,
//! schema, and wording in one request. The classifier answers exactly one
//! question with a grammar-forced enum. Drafting is plain text with no JSON.

use serde_json::{Value, json};

/// The operation enums the classifier may choose from.
const OPERATIONS: [&str; 7] = [
    "list_recent_messages",
    "list_unread_messages",
    "get_thread",
    "search_messages",
    "summarize_thread",
    "prepare_reply",
    "send_reply",
];

/// The one-question system prompt for tier 1 (intent classification).
#[must_use]
pub fn classifier_system() -> String {
    format!(
        r#"You classify one user request into exactly one messaging action.
Answer ONLY this JSON, nothing else:
{{"operation": "<enum>", "needs_clarification": <true|false>}}

Enums and when to use them:
- "send_reply": the user's main verb is send/respond AND the final outbound text is stated
  verbatim, for example: Send exactly: "Thanks, I'll review this."
- "prepare_reply": a reply is wanted but it is a draft, or the user only describes what the
  reply should convey ("draft", "prepare", "write back", "draft a reply saying I'm interested").
- "summarize_thread": condense an existing conversation or message.
- "search_messages": locate messages by topic, sender, or timing.
- "get_thread": open or read one specific conversation.
- "list_unread_messages": check for new/unread/important items.
- "list_recent_messages": show recent activity or latest messages.

Rules:
- Reading/checking questions are "list_unread_messages" first, "list_recent_messages" second.
- If the request is not about the user's messages, set needs_clarification to true and
  operation "list_unread_messages" (a harmless read).
- If it is unclear who or what the action applies to, set needs_clarification to true.
- "draft ... saying X" is "prepare_reply", never "send_reply".

Allowed operation values: {OPERATIONS:?}"#,
    )
}

/// JSON schema the classifier grammar is constrained to: one enum + one boolean.
#[must_use]
pub fn classifier_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["operation", "needs_clarification"],
        "properties": {
            "operation": {"type": "string", "enum": OPERATIONS},
            "needs_clarification": {"type": "boolean"}
        }
    })
}

/// System prompt for tier 3 (drafting) — plain text, no JSON.
///
/// The thread context is supplied by Rust and is already untrusted-marked, so
/// injection inside a message cannot become an instruction here either.
#[must_use]
pub fn draft_system() -> String {
    r#"You write one short reply for the user.
Given: who the reply is for and what the user asked to convey.
Write only the reply body. No subject, no signature, no commentary, no JSON.
Be concise: 1-3 sentences, plain language, match the user's tone ("polite", "brief")."#
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Operation;

    #[test]
    fn classifier_enum_matches_runtime_operations() {
        for name in OPERATIONS {
            let parsed: Operation = serde_json::from_str(&format!("\"{name}\""))
                .unwrap_or_else(|error| panic!("{name} is not a valid Operation: {error}"));
            assert_eq!(parsed.as_str(), name);
        }
    }

    #[test]
    fn classifier_schema_covers_every_operation() {
        let schema = classifier_schema();
        let allowed = schema["properties"]["operation"]["enum"]
            .as_array()
            .expect("enum present")
            .iter()
            .filter_map(|value| value.as_str())
            .collect::<Vec<_>>();
        assert_eq!(allowed.len(), OPERATIONS.len());
        for name in OPERATIONS {
            assert!(allowed.contains(&name), "missing {name}");
        }
    }
}
