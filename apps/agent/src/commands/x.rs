//! `herald x login` — human sign-in on the dedicated X profile.
//!
//! This is the only command that opens a visible browser window. Herald never
//! reads cookies, never types credentials, and never completes a challenge
//! itself: the user signs in, and Herald only waits until the home timeline
//! reports an authenticated session.

use std::time::Duration;

use serde_json::json;

use herald_provider::ProviderError;
use herald_provider::x::{XConfig, login as open_login_window};

use crate::app::App;
use crate::error::AppError;

/// How long the sign-in window waits before giving up.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(600);

pub async fn login(app: &App) -> Result<u8, AppError> {
    let settings = app.config.x.as_ref().ok_or_else(|| {
        AppError::new(
            "not_configured",
            "no [x] section in config.toml; add one (see `herald config init`)",
        )
    })?;

    let profile_dir = app.config.x_profile_dir(&app.data_dir);
    if herald_provider::x::detect_chrome().is_none() {
        return Err(AppError::new(
            "browser_missing",
            "Chrome or Chromium was not found; install it, or set CHROME to the executable",
        ));
    }
    std::fs::create_dir_all(&profile_dir).map_err(|error| {
        AppError::new(
            "io",
            format!("creating the X profile {}: {error}", profile_dir.display()),
        )
    })?;

    app.out
        .line("Opening a visible Chrome window on the dedicated X profile.");
    app.out.field("profile", profile_dir.display());
    app.out
        .line("Sign in yourself. Herald does not read cookies or type credentials.");
    app.out
        .line("The window closes when a signed-in home timeline is detected.");

    let config = XConfig {
        profile_dir: profile_dir.clone(),
        handle: settings.handle.clone(),
        headless: settings.headless,
        action_delay_ms: settings.action_delay_ms,
        typing_delay_ms: settings.typing_delay_ms,
        close_after: true,
        browser_executable: None,
    };

    match open_login_window(&config, LOGIN_TIMEOUT).await {
        Ok(()) => {
            app.out.blank();
            app.out
                .line("signed in: the X adapter can read this profile.");
            app.out.data(
                "x_login",
                json!({
                    "status": "authenticated",
                    "provider": "x",
                    "profile": profile_dir.display().to_string(),
                }),
            );
            Ok(0)
        }
        Err(ProviderError::Timeout) => Err(AppError::new(
            "timeout",
            "no signed-in home timeline appeared within 10 minutes; nothing was stored",
        )),
        Err(error) => Err(AppError::new("x_login", error.to_string())),
    }
}
