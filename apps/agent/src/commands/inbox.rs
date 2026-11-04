//! `herald inbox` and `herald thread` — direct reads without an agent action.

use serde_json::json;

use herald_core::{MessageQuery, MessageScope, ThreadId};

use crate::app::App;
use crate::commands::{ask, resolve_provider};
use crate::error::AppError;

pub async fn inbox(
    app: &App,
    scope: MessageScope,
    unread: bool,
    limit: usize,
    provider_override: Option<&str>,
) -> Result<u8, AppError> {
    let (provider_id, adapter) = resolve_provider(app, provider_override)?;
    let adapter = adapter.ok_or_else(|| {
        AppError::new(
            "no_adapter",
            format!("no {provider_id} adapter is implemented in this build"),
        )
    })?;

    let mut query = if unread {
        MessageQuery::unread(limit)
    } else {
        MessageQuery::recent(limit)
    };
    query.scope = scope;
    let messages = adapter.list_recent(&query).await?;

    if messages.is_empty() {
        app.out.line("No messages.");
    } else {
        ask::print_messages(app.out, &messages);
    }

    app.out.data(
        "inbox",
        json!({
            "provider": provider_id,
            "scope": scope,
            "unread_only": unread,
            "messages": ask::messages_json(&messages),
            "count": messages.len(),
        }),
    );
    Ok(0)
}

pub async fn thread(app: &App, id: &str, provider_override: Option<&str>) -> Result<u8, AppError> {
    let (provider_id, adapter) = resolve_provider(app, provider_override)?;
    let adapter = adapter.ok_or_else(|| {
        AppError::new(
            "no_adapter",
            format!("no {provider_id} adapter is implemented in this build"),
        )
    })?;

    let thread = adapter.get_thread(&ThreadId::new(id)).await?;
    app.out.field("subject", &thread.subject);
    app.out.field("messages", thread.messages.len());
    ask::print_thread(app.out, &thread);

    app.out.data("thread", ask::thread_json(&thread));
    Ok(0)
}
