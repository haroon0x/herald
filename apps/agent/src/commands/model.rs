//! `herald model` — download, verify, pin, and inspect the model artifact.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::json;
use sha2::{Digest, Sha256};

use herald_config::ModelBackend;
use herald_runtime::ModelManifest;

use crate::app::App;
use crate::error::AppError;

pub fn status(app: &App) -> u8 {
    let backend = app.config.model.backend;
    app.out.field("backend", backend.as_str());

    match backend {
        ModelBackend::None => {
            app.out
                .line("No model is configured; deterministic rules and explicit bodies are used.");
        }
        ModelBackend::OpenAi => {
            app.out.field(
                "endpoint",
                app.config.model.endpoint.as_deref().unwrap_or("?"),
            );
            app.out.field(
                "model",
                app.config.model.model_name.as_deref().unwrap_or("?"),
            );
        }
        ModelBackend::OpenCode => {
            app.out.field(
                "model",
                app.config
                    .model
                    .model_name
                    .as_deref()
                    .unwrap_or(herald_runtime::opencode::DEFAULT_MODEL),
            );
            app.out.field(
                "binary",
                std::env::var("HERALD_OPENCODE_PATH").unwrap_or_else(|_| "opencode".to_string()),
            );
            app.out.field(
                "sandbox",
                app.data_dir.join("opencode-sandbox").display().to_string(),
            );
            app.out
                .line("One-shot `opencode run --standalone --model <id>` per completion; --auto is never passed.");
        }
        ModelBackend::Managed => {
            let manifest_path = manifest_path(app, None);
            app.out.field("manifest", manifest_path.display());
            match ModelManifest::load(&manifest_path) {
                Ok(manifest) => {
                    app.out.field("name", &manifest.name);
                    app.out.field("quantization", &manifest.quantization);
                    app.out.field("revision", &manifest.revision);
                    let file = model_file(app, &manifest);
                    if file.exists() {
                        let size = std::fs::metadata(&file).map_or(0, |meta| meta.len());
                        app.out
                            .field("file", format!("{} ({size} bytes)", file.display()));
                    } else {
                        app.out
                            .field("file", format!("{} (missing)", file.display()));
                    }
                    app.out.field("sha256", truncate_hash(&manifest.sha256));
                }
                Err(error) => {
                    app.out.field("manifest", format!("unreadable: {error}"));
                }
            }
        }
    }

    app.out.data(
        "model.status",
        json!({
            "backend": backend.as_str(),
            "endpoint": app.config.model.endpoint,
            "model_name": app.config.model.model_name,
            "manifest": (backend == ModelBackend::Managed).then(|| manifest_path(app, None)),
        }),
    );
    0
}

pub async fn download(app: &App) -> Result<u8, AppError> {
    let manifest_path = manifest_path(app, None);
    let manifest = ModelManifest::load(&manifest_path).map_err(|error| {
        AppError::new(
            "model",
            format!("loading {}: {error}", manifest_path.display()),
        )
    })?;

    if manifest.sha256.starts_with("PIN_") || manifest.sha256.len() != 64 {
        return Err(AppError::new(
            "model_not_pinned",
            "the manifest has no real SHA-256; run `herald model pin <file>` first",
        )
        .with_hint("A placeholder digest is refused so an unverified model cannot be loaded."));
    }

    let url = std::env::var("HERALD_MODEL_URL")
        .ok()
        .filter(|value| !value.trim().is_empty());

    match herald_runtime::download_model(&manifest, &app.models_dir, url).await {
        Ok(None) => {
            app.out
                .field("model", format!("{} (already present)", manifest.filename));
        }
        Ok(Some(stats)) => {
            app.out
                .field("download", format!("{} bytes", stats.bytes_written));
            app.out.field("elapsed", format!("{} ms", stats.elapsed_ms));
            app.out.field("resumed", stats.resumed);
        }
        Err(error) => return Err(AppError::new("model", format!("download failed: {error}"))),
    }

    app.out.data(
        "model.download",
        json!({"file": manifest.filename, "verified": true}),
    );
    Ok(0)
}

pub fn verify(app: &App) -> Result<u8, AppError> {
    let manifest_path = manifest_path(app, None);
    let manifest = ModelManifest::load(&manifest_path).map_err(|error| {
        AppError::new(
            "model",
            format!("loading {}: {error}", manifest_path.display()),
        )
    })?;
    let file = model_file(app, &manifest);
    manifest
        .verify(&file)
        .map_err(|error| AppError::new("model", error.to_string()))?;

    app.out.field("file", file.display());
    app.out.field("sha256", &manifest.sha256);
    app.out.line("checksum ok");
    app.out.data(
        "model.verify",
        json!({"file": file, "sha256": manifest.sha256, "verified": true}),
    );
    Ok(0)
}

#[allow(clippy::too_many_arguments)]
pub fn pin(
    app: &App,
    file: &Path,
    name: &str,
    repo: &str,
    revision: &str,
    quantization: &str,
    manifest_override: Option<&Path>,
) -> Result<u8, AppError> {
    if !file.is_file() {
        return Err(AppError::not_found(format!(
            "{} is not a file",
            file.display()
        )));
    }

    let sha256 = sha256_file(file)?;
    let size_bytes = std::fs::metadata(file)
        .map_err(|error| AppError::new("io", error.to_string()))?
        .len();
    let absolute = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());

    let manifest = ModelManifest {
        name: name.to_string(),
        repo: repo.to_string(),
        revision: revision.to_string(),
        filename: absolute.display().to_string(),
        quantization: quantization.to_string(),
        license: "see upstream".to_string(),
        sha256: sha256.clone(),
        size_bytes,
    };

    let path =
        manifest_override.map_or_else(|| app.models_dir.join("manifest.json"), Path::to_path_buf);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| AppError::new("io", error.to_string()))?;
    }
    let serialized = serde_json::to_string_pretty(&manifest)
        .map_err(|error| AppError::new("io", error.to_string()))?;
    std::fs::write(&path, format!("{serialized}\n"))
        .map_err(|error| AppError::new("io", error.to_string()))?;

    app.out.field("file", absolute.display());
    app.out.field("sha256", &sha256);
    app.out.field("size", size_bytes);
    app.out.field("manifest", path.display());
    app.out.data(
        "model.pin",
        json!({
            "file": absolute,
            "sha256": sha256,
            "size_bytes": size_bytes,
            "manifest": path,
        }),
    );
    Ok(0)
}

fn manifest_path(app: &App, explicit: Option<&Path>) -> PathBuf {
    explicit
        .map(Path::to_path_buf)
        .or_else(|| app.config.model.manifest.clone())
        .unwrap_or_else(|| app.models_dir.join("manifest.json"))
}

fn model_file(app: &App, manifest: &ModelManifest) -> PathBuf {
    let path = Path::new(&manifest.filename);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        app.models_dir.join(path)
    }
}

fn sha256_file(path: &Path) -> Result<String, AppError> {
    let mut file = std::fs::File::open(path)
        .map_err(|error| AppError::new("io", format!("opening {}: {error}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| AppError::new("io", error.to_string()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn truncate_hash(hash: &str) -> String {
    if hash.len() <= 16 {
        hash.to_string()
    } else {
        format!("{}…", &hash[..16])
    }
}
