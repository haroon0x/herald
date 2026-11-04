//! `herald config` — read the effective configuration without exposing secrets.

use std::path::PathBuf;

use serde_json::{Map, Value, json};

use crate::app::App;
use crate::cli::Cli;
use crate::error::AppError;
use crate::output::Out;

/// Resolve the config path without requiring the file to exist.
#[must_use]
pub fn resolved_path(cli: &Cli) -> PathBuf {
    if let Some(path) = &cli.config {
        return path.clone();
    }
    if let Some(raw) = std::env::var_os("HERALD_CONFIG") {
        return PathBuf::from(raw);
    }
    herald_config::default_config_path()
}

pub fn print_path(cli: &Cli) {
    let out = Out::new();
    let path = resolved_path(cli);
    out.line(path.display().to_string());
    out.data(
        "config.path",
        json!({"path": path, "exists": path.exists()}),
    );
}

pub fn init(cli: &Cli, force: bool) -> Result<(), AppError> {
    let out = Out::new();
    let path = resolved_path(cli);
    herald_config::write_template(&path, force)?;
    out.field("config", path.display());
    out.line("Edit the file, then run `herald doctor`.");
    out.data("config.init", json!({"path": path, "written": true}));
    Ok(())
}

pub fn init_for_app(app: &App, force: bool) -> Result<u8, AppError> {
    herald_config::write_template(&app.config_path, force)?;
    app.out.field("config", app.config_path.display());
    app.out.data(
        "config.init",
        json!({"path": app.config_path, "written": true}),
    );
    Ok(0)
}

pub fn path(app: &App) -> u8 {
    app.out.line(app.config_path.display().to_string());
    app.out.data(
        "config.path",
        json!({"path": app.config_path, "exists": app.config_exists}),
    );
    0
}

pub fn get(app: &App, key: &str) -> Result<u8, AppError> {
    let Some(value) = app.config.get(key) else {
        return Err(AppError::not_found(format!(
            "unknown configuration key: {key}"
        )));
    };
    app.out.line(value.clone());
    app.out
        .data("config.get", json!({"key": key, "value": value}));
    Ok(0)
}

pub fn list(app: &App) -> u8 {
    let flat = app.config.flat();
    for (key, value) in &flat {
        app.out.line(format!("{key} = {value}"));
    }

    let mut map = Map::new();
    for (key, value) in flat {
        map.insert(key, Value::String(value));
    }
    app.out
        .data("config.list", json!({"config": Value::Object(map)}));
    0
}
