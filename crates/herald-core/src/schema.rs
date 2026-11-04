//! The JSON schema the planner must emit.
//!
//! This is the single source of truth for `ActionPlan`'s shape. The inference
//! runtime grammar-constrains generation with it, so a small local model cannot
//! invent keys, providers, or operations.

use serde_json::{Value, json};

/// Full JSON Schema for one `ActionPlan` object.
#[must_use]
pub fn action_plan_schema() -> Value {
    let operation = json!({
        "type": "string",
        "enum": [
            "list_recent_messages",
            "list_unread_messages",
            "get_thread",
            "search_messages",
            "summarize_thread",
            "prepare_reply",
            "send_reply"
        ]
    });

    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "ActionPlan",
        "type": "object",
        "additionalProperties": false,
        "required": ["provider", "operation", "target"],
        "properties": {
            "provider": {
                "type": "string",
                "enum": ["gmail", "x", "fake", "maildir"]
            },
            "operation": operation,
            "target": {
                "type": "object",
                "oneOf": [
                    {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["kind", "description"],
                        "properties": {
                            "kind": {"enum": ["description"]},
                            "description": {"type": "string"}
                        }
                    },
                    {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["kind", "thread_id"],
                        "properties": {
                            "kind": {"enum": ["thread"]},
                            "thread_id": {"type": "string"}
                        }
                    },
                    {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["kind", "sender"],
                        "properties": {
                            "kind": {"enum": ["latest_from"]},
                            "sender": {"type": "string"}
                        }
                    }
                ]
            },
            "reply_body": {"type": "string"},
            "needs_clarification": {"type": "boolean"},
            "clarification_question": {"type": "string"}
        }
    })
}

#[cfg(test)]
mod tests {
    use serde_json::from_str;

    #[test]
    fn schema_accepts_a_valid_plan() {
        let plan = r#"{
            "provider": "gmail",
            "operation": "send_reply",
            "target": {"kind": "thread", "thread_id": "t1"},
            "reply_body": "ok"
        }"#;
        let value: serde_json::Value = from_str(plan).unwrap();
        let schema = super::action_plan_schema();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let errors: Vec<String> = validator
            .iter_errors(&value)
            .map(|error| error.to_string())
            .collect();
        assert!(validator.is_valid(&value), "schema rejected: {errors:?}");
    }
}
