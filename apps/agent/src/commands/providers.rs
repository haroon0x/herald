//! `herald providers` — what this build can actually talk to.

use serde_json::json;

use herald_core::ProviderId;
use herald_provider::ProviderHealth;

use crate::app::App;
use crate::error::AppError;

struct Row {
    id: ProviderId,
    status: &'static str,
    detail: String,
    is_default: bool,
}

pub async fn run(app: &App) -> Result<u8, AppError> {
    // Health checks run concurrently: a slow provider (a browser launch, a
    // network timeout) must not hold the others hostage. Rows stay in
    // `ProviderId::ALL` order.
    let mut futures = Vec::new();
    for id in ProviderId::ALL {
        futures.push(async move {
            let is_default = id == app.config.default_provider;
            match app.registry.get(id) {
                Some(adapter) => {
                    let (status, detail) = match adapter.health().await {
                        ProviderHealth::Ready => ("ready", "adapter available".to_string()),
                        ProviderHealth::NotAuthenticated => (
                            "not authenticated",
                            "sign in or configure credentials".to_string(),
                        ),
                        ProviderHealth::Unverified(reason) => {
                            ("unverified", format!("{reason}; a read may still work"))
                        }
                        ProviderHealth::Unavailable(reason) => ("unavailable", reason),
                    };
                    Row {
                        id,
                        status,
                        detail,
                        is_default,
                    }
                }
                None if id.is_implemented() => Row {
                    id,
                    status: "not configured",
                    detail: "adapter exists but is not configured; run `herald init`".to_string(),
                    is_default,
                },
                None => Row {
                    id,
                    status: "planned",
                    detail: "no adapter in this build; plans are inert".to_string(),
                    is_default,
                },
            }
        });
    }
    let mut rows = Vec::with_capacity(futures.len());
    for future in futures {
        rows.push(future.await);
    }

    app.out.line(format!(
        "{:<10} {:<18} DEFAULT  DETAIL",
        "PROVIDER", "STATUS"
    ));
    for row in &rows {
        app.out.line(format!(
            "{:<10} {:<18} {:<8} {}",
            row.id.as_str(),
            row.status,
            if row.is_default { "*" } else { "" },
            row.detail
        ));
    }

    let items = rows
        .iter()
        .map(|row| {
            json!({
                "provider": row.id,
                "status": row.status,
                "detail": row.detail,
                "implemented": row.id.is_implemented(),
                "default": row.is_default,
            })
        })
        .collect::<Vec<_>>();
    app.out.data("providers", json!({"providers": items}));
    Ok(0)
}
