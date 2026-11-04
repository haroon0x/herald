//! `herald history` and `herald audit`.

use serde_json::json;

use crate::app::App;
use crate::error::AppError;
use crate::output::format_time_ms;

pub fn history(app: &App, limit: usize) -> Result<u8, AppError> {
    let actions = app.store.recent_actions(limit)?;

    if actions.is_empty() {
        app.out.line("No recorded actions.");
        app.out.data("history", json!({"actions": [], "count": 0}));
        return Ok(0);
    }

    app.out.line(format!(
        "{:<36} {:<24} {:<12} {:<18} TARGET",
        "ACTION", "OPERATION", "STATE", "PROVIDER"
    ));
    for action in &actions {
        app.out.line(format!(
            "{:<36} {:<24} {:<12} {:<18} {}",
            action.action_id,
            action.operation,
            action.state.as_str(),
            action.provider.as_str(),
            action.target
        ));
    }

    let items: Vec<_> = actions
        .iter()
        .map(|action| {
            json!({
                "action_id": action.action_id,
                "provider": action.provider,
                "operation": action.operation,
                "target": action.target,
                "thread_id": action.thread_id,
                "state": action.state.as_str(),
                "created_at_ms": action.created_at_ms,
                "completed_at_ms": action.completed_at_ms,
                "content_hash": action.content_hash,
            })
        })
        .collect();
    app.out
        .data("history", json!({"actions": items, "count": actions.len()}));
    Ok(0)
}

pub fn audit(app: &App, limit: usize) -> Result<u8, AppError> {
    let events = app.store.recent_audit(limit)?;

    if events.is_empty() {
        app.out.line("No audit events.");
        app.out.data("audit", json!({"events": [], "count": 0}));
        return Ok(0);
    }

    for event in &events {
        app.out.line(format!(
            "{}  {:<20} {:<36} {}",
            format_time_ms(event.at_ms),
            event.status,
            event.action_id.as_deref().unwrap_or("-"),
            event.detail
        ));
    }

    let items: Vec<_> = events
        .iter()
        .map(|event| {
            json!({
                "action_id": event.action_id,
                "at_ms": event.at_ms,
                "status": event.status,
                "detail": event.detail,
            })
        })
        .collect();
    app.out
        .data("audit", json!({"events": items, "count": events.len()}));
    Ok(0)
}
