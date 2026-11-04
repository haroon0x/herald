//! `herald init` — create paths, database, and a starter config.

use serde_json::json;

use herald_core::time::now_ms;
use herald_storage::{AuditEvent, Store};

use crate::app::{maybe_write_config, prepare_paths};
use crate::cli::Cli;
use crate::error::AppError;
use crate::output::Out;

/// The manifest that ships with the binary. Written on init so the default
/// model can be downloaded without the repository checkout.
const BUNDLED_MANIFEST: &str = include_str!("../../../../models/manifest.json");

pub fn run(cli: &Cli, force: bool) -> Result<(), AppError> {
    let out = Out::new();
    let (data_dir, config_path) = prepare_paths(cli)?;
    let db_path = data_dir.join("herald.db");
    let store = Store::open(&db_path)?;
    let wrote_config = maybe_write_config(&config_path, force)?;

    let models_dir = data_dir.join("models");
    std::fs::create_dir_all(&models_dir).map_err(|error| {
        AppError::new("io", format!("creating {}: {error}", models_dir.display()))
    })?;
    let manifest_path = models_dir.join("manifest.json");
    let wrote_manifest = if manifest_path.exists() {
        false
    } else {
        std::fs::write(&manifest_path, BUNDLED_MANIFEST).map_err(|error| {
            AppError::new(
                "io",
                format!("writing {}: {error}", manifest_path.display()),
            )
        })?;
        true
    };

    store.append_audit(&AuditEvent {
        action_id: None,
        at_ms: now_ms(),
        status: "init".into(),
        detail: format!(
            "data={}, config={}, config_written={wrote_config}",
            data_dir.display(),
            config_path.display()
        ),
    })?;

    out.heading(format!("Herald {}", env!("CARGO_PKG_VERSION")));
    out.field("data", data_dir.display());
    out.field("database", db_path.display());
    out.field(
        "config",
        format!(
            "{} ({})",
            config_path.display(),
            if wrote_config { "written" } else { "kept" }
        ),
    );
    out.field(
        "manifest",
        format!(
            "{} ({})",
            manifest_path.display(),
            if wrote_manifest { "written" } else { "kept" }
        ),
    );
    out.blank();
    out.line("Nothing is connected yet. Try the demo inbox:");
    out.line("  herald ask \"check what is unread\"");
    out.line(
        "  herald ask 'send exactly: \"Thanks, I will review this.\" to the founder' --enable-send",
    );
    out.blank();
    out.line("To read real mail, edit the [maildir] section in the config, then run:");
    out.line("  herald doctor");

    out.data(
        "init",
        json!({
            "data_dir": data_dir,
            "database": db_path,
            "config": config_path,
            "config_written": wrote_config,
            "manifest": manifest_path,
            "manifest_written": wrote_manifest,
        }),
    );
    Ok(())
}
