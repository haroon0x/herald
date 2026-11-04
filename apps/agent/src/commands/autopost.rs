//! `herald autopost status|on|off` — the scoped autopost lane.
//!
//! The kill switch is a file (`<data-dir>/autopost.disabled`), not a config
//! edit, so turning autonomous posting off is immediate, scriptable, and does
//! not race a running command. Turning it back on cannot re-enable the lane by
//! itself: `[policy] allow_autopost` must also be true, and every other rail
//! still has to pass.

use serde_json::json;

use herald_config::quiet_hours_active;

use crate::app::App;
use crate::cli::AutopostAction;
use crate::commands::{AUTOPOST_KILL_SWITCH, autopost_remaining, day_start_ms, minutes_of_day_utc};
use crate::error::AppError;

pub fn run(app: &App, action: AutopostAction) -> Result<u8, AppError> {
    match action {
        AutopostAction::Status => Ok(status(app)),
        AutopostAction::On => set_switch(app, false),
        AutopostAction::Off => set_switch(app, true),
    }
}

fn kill_switch_path(app: &App) -> std::path::PathBuf {
    app.data_dir.join(AUTOPOST_KILL_SWITCH)
}

fn status(app: &App) -> u8 {
    let now = herald_core::time::now_ms();
    let settings = &app.config.policy;
    let kill_switch = kill_switch_path(app).exists();
    let quiet = settings
        .autopost_quiet_hours
        .as_deref()
        .is_some_and(|hours| quiet_hours_active(hours, minutes_of_day_utc(now)));
    let remaining = autopost_remaining(app, now);
    let used = settings.autopost_daily_limit.saturating_sub(remaining);
    let configured = settings.allow_autopost;
    let effective = configured && !kill_switch && remaining > 0 && !quiet;

    app.out.heading("Autopost");
    app.out
        .field("configured", if configured { "on" } else { "off" });
    app.out
        .field("kill switch", if kill_switch { "on" } else { "off" });
    app.out.field(
        "quiet hours",
        settings.autopost_quiet_hours.as_deref().map_or_else(
            || "none".to_string(),
            |hours| {
                format!(
                    "{hours} UTC ({})",
                    if quiet { "active" } else { "inactive" }
                )
            },
        ),
    );
    app.out.field(
        "daily budget",
        format!("{used}/{} used today (UTC)", settings.autopost_daily_limit),
    );
    app.out.field("remaining", format!("{remaining} post(s)"));
    app.out.field(
        "quality floor",
        format!("{}/10", settings.autopost_min_quality),
    );
    app.out.field(
        "send path",
        if effective {
            "autopost"
        } else {
            "approval only"
        },
    );
    app.out.blank();
    if effective {
        app.out
            .line("Original posts may publish without approval while every rail holds.");
        app.out
            .line("DMs and replies never autopost; they always need `herald approve`.");
    } else {
        app.out
            .line("Everything still goes through the approval queue. Nothing autoposts.");
        if !configured {
            app.out
                .line("Enable it with [policy] allow_autopost = true (and allow_send).");
        }
        if kill_switch {
            app.out
                .line("The kill switch is on; clear it with `herald autopost on`.");
        }
    }

    app.out.data(
        "autopost",
        json!({
            "status": if effective { "enabled" } else { "approval_only" },
            "configured": configured,
            "kill_switch": kill_switch,
            "quiet_hours": settings.autopost_quiet_hours,
            "quiet_now": quiet,
            "daily_limit": settings.autopost_daily_limit,
            "used_today": used,
            "remaining_today": remaining,
            "min_quality": settings.autopost_min_quality,
            "today_start_ms": day_start_ms(now),
        }),
    );
    0
}

fn set_switch(app: &App, disabled: bool) -> Result<u8, AppError> {
    let path = kill_switch_path(app);
    if disabled {
        let body = format!(
            "autopost disabled by `herald autopost off` at {} (unix ms)\n",
            herald_core::time::now_ms()
        );
        std::fs::write(&path, body).map_err(|error| {
            AppError::new(
                "io",
                format!("writing the kill switch {}: {error}", path.display()),
            )
        })?;
        app.out
            .line("autopost is off: every post now needs `herald approve`.");
    } else if path.exists() {
        std::fs::remove_file(&path).map_err(|error| {
            AppError::new(
                "io",
                format!("removing the kill switch {}: {error}", path.display()),
            )
        })?;
        app.out.line("kill switch cleared.");
    } else {
        app.out.line("kill switch was already clear.");
    }

    if !app.config.policy.allow_autopost {
        app.out
            .line("note: [policy] allow_autopost is false, so posts still require approval.");
    }
    if !app.config.policy.allow_send {
        app.out
            .line("note: [policy] allow_send is false, so no outbound action can be approved.");
    }

    app.out.data(
        "autopost",
        json!({
            "status": if disabled { "disabled" } else { "kill_switch_cleared" },
            "kill_switch": disabled,
            "configured": app.config.policy.allow_autopost,
            "path": path.display().to_string(),
        }),
    );
    Ok(0)
}
