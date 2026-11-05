//! Herald CLI entry point.

mod app;
mod cli;
mod commands;
mod error;
mod output;
mod planner;
mod registry;
mod send;
mod ui;

use std::process::ExitCode;

use clap::Parser;

use crate::cli::Cli;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    output::init(&cli);
    let log = cli
        .log
        .clone()
        .or_else(|| std::env::var("HERALD_LOG").ok())
        .unwrap_or_else(|| "warn".to_string());
    init_tracing(&log);

    match app::run(cli).await {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            error.report();
            ExitCode::from(error.exit_code())
        }
    }
}

/// Third-party crates whose debug logs can contain page text, headers, or
/// cookies. Herald's own targets keep whatever level the user asked for.
const LOG_CAPS: [&str; 11] = [
    // Its warnings are parse noise for CDP events Chrome added after the
    // crate's schema; only hard errors are useful.
    "chromiumoxide",
    "html5ever",
    "markup5ever",
    "tendril",
    "scraper",
    "hyper",
    "hyper_util",
    "reqwest",
    "tungstenite",
    "async_tungstenite",
    "h2",
];

fn log_filter(filter: &str) -> String {
    use std::fmt::Write as _;
    let mut filter = filter.to_string();
    for target in LOG_CAPS {
        let level = if target == "chromiumoxide" {
            "error"
        } else {
            "warn"
        };
        let _ = write!(filter, ",{target}={level}");
    }
    filter
}

fn init_tracing(filter: &str) {
    // chromiumoxide logs raw CDP messages at debug, including request headers
    // with session cookies. Cap its target regardless of the user filter so a
    // debugging session cannot print `auth_token` or `ct0`.
    let filter = log_filter(filter);
    let filter = tracing_subscriber::EnvFilter::try_new(filter)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,chromiumoxide=warn"));

    // Logging goes to stderr and is optional; a failed initialization (for
    // example in tests) must not change command behavior.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .without_time()
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracing_caps_chromiumoxide_below_debug() {
        let filter = log_filter("debug");
        for target in LOG_CAPS {
            let level = if target == "chromiumoxide" {
                "error"
            } else {
                "warn"
            };
            assert!(filter.contains(&format!("{target}={level}")), "{target}");
        }
        assert!(tracing_subscriber::EnvFilter::try_new(filter).is_ok());
    }
}
