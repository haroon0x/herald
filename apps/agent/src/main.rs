//! Herald CLI entry point.

mod app;
mod cli;
mod commands;
mod error;
mod output;
mod planner;
mod registry;
mod send;

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

fn init_tracing(filter: &str) {
    let filter = tracing_subscriber::EnvFilter::try_new(filter)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));

    // Logging goes to stderr and is optional; a failed initialization (for
    // example in tests) must not change command behavior.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .without_time()
        .try_init();
}
