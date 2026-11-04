//! `herald approvals`, `approve`, `reject`, and `reconcile`.

use serde_json::json;

use herald_core::ActionState;
use herald_storage::PendingApproval;

use crate::app::App;
use crate::error::AppError;
use crate::output::format_time_ms;
use crate::send;

pub fn list(app: &App) -> Result<u8, AppError> {
    let mut rows = Vec::new();
    for approval in app.store.pending_approvals()? {
        let Some(action) = app.store.get_action(&approval.action_id)? else {
            continue;
        };
        if action.state != ActionState::AwaitingApproval {
            continue;
        }
        rows.push((approval, action));
    }

    if rows.is_empty() {
        app.out.line("No approvals are waiting.");
        app.out
            .data("approvals", json!({"approvals": [], "count": 0}));
        return Ok(0);
    }

    app.out
        .heading(format!("{} approval(s) waiting", rows.len()));
    for (approval, action) in &rows {
        app.out.blank();
        app.out.field("action", &approval.action_id);
        app.out.field("provider", approval.provider);
        app.out.field("target", &action.target);
        app.out.field("thread", &approval.thread_id);
        app.out.field("body", preview(&approval.body, 72));
        app.out.field("hash", &approval.body_hash);
        app.out
            .field("expires", format_time_ms(approval.expires_at_ms));
    }
    app.out.blank();
    app.out
        .line("Approve exactly one body with `herald approve <action-id>`; reject with `herald reject <action-id>`.");

    let items = rows
        .iter()
        .map(|(approval, action)| approval_json(approval, &action.target))
        .collect::<Vec<_>>();
    app.out.data(
        "approvals",
        json!({"approvals": items, "count": rows.len()}),
    );
    Ok(0)
}

pub async fn approve(app: &App, id: &str) -> Result<u8, AppError> {
    // Show the exact bytes again immediately before the irreversible step.
    if let Some(approval) = app.store.get_approval(id)? {
        app.out.field("target", approval.target.describe());
        app.out.field("thread", &approval.thread_id);
        app.out.field("body", &approval.body);
        app.out.field("hash", &approval.body_hash);
    }

    let outcome = send::approve(app, id).await?;

    app.out.field("action", id);
    app.out.field("status", outcome.status);
    app.out.field("detail", &outcome.detail);
    if let Some(message_id) = &outcome.message_id {
        app.out.field("message", message_id);
    }

    if outcome.status == "uncertain" {
        app.out.blank();
        app.out
            .line(format!("Resolve it with: herald reconcile {id}"));
    }

    app.out.data(
        "approve",
        json!({
            "action_id": id,
            "status": outcome.status,
            "verified": outcome.verified,
            "message_id": outcome.message_id,
            "detail": outcome.detail,
        }),
    );
    Ok(0)
}

pub fn reject(app: &App, id: &str) -> Result<u8, AppError> {
    send::reject(app, id)?;
    app.out.field("action", id);
    app.out.field("status", "cancelled");
    app.out.line("Nothing was sent.");
    app.out.data(
        "reject",
        json!({"action_id": id, "status": "cancelled", "sent": false}),
    );
    Ok(0)
}

pub async fn reconcile(app: &App, id: &str) -> Result<u8, AppError> {
    let outcome = send::reconcile(app, id).await?;

    app.out.field("action", id);
    app.out.field("status", outcome.status);
    app.out.field("detail", &outcome.detail);
    if let Some(message_id) = &outcome.message_id {
        app.out.field("message", message_id);
    }

    app.out.data(
        "reconcile",
        json!({
            "action_id": id,
            "status": outcome.status,
            "verified": outcome.verified,
            "message_id": outcome.message_id,
            "detail": outcome.detail,
        }),
    );
    Ok(0)
}

fn approval_json(approval: &PendingApproval, target: &str) -> serde_json::Value {
    json!({
        "action_id": approval.action_id,
        "provider": approval.provider,
        "target": target,
        "thread_id": approval.thread_id,
        "body": approval.body,
        "body_hash": approval.body_hash,
        "created_at_ms": approval.created_at_ms,
        "expires_at_ms": approval.expires_at_ms,
    })
}

fn preview(body: &str, limit: usize) -> String {
    let one_line = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= limit {
        one_line
    } else {
        let truncated: String = one_line.chars().take(limit.saturating_sub(1)).collect();
        format!("{truncated}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_collapses_whitespace_and_truncates_on_char_boundaries() {
        assert_eq!(preview("hello\n\nworld", 72), "hello world");
        let long = "é".repeat(100);
        let rendered = preview(&long, 10);
        assert_eq!(rendered.chars().count(), 10);
        assert!(rendered.ends_with('…'));
    }
}
