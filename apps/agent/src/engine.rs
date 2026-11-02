//! Planner engines for a tiered model.
//!
//! Tier 0 (rules) is in `herald-core::rules`. Tier 1 (intent classification) and
//! tier 3 (drafting) live here. Tier 2 (target resolution) is never done by the
//! model: the provider adapter resolves a description against real data.

use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use herald_core::{ActionPlan, Operation, rules};
use herald_runtime::{LlamaSidecar, ModelManifest, SidecarConfig};

pub const CLASSIFY_MAX_TOKENS: u16 = 32;
pub const RETRY_MAX_TOKENS: u16 = 512;
pub const DRAFT_MAX_TOKENS: u16 = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    /// Rules only; never touches the model.
    Rules,
    /// Rules first, model classifier fallback on a rules miss.
    Auto,
    /// Model classifier only (used by evals to measure model-only quality).
    Model,
}

#[derive(Debug)]
pub struct ModelSetup {
    sidecar: LlamaSidecar,
}

impl ModelSetup {
    /// Start the managed sidecar from a manifest and models directory.
    pub async fn start(manifest_path: &Path, models_dir: &Path) -> Result<Self> {
        let manifest = ModelManifest::load(manifest_path)
            .map_err(|error| anyhow::anyhow!("loading model manifest: {error}"))?;
        let model_file = models_dir.join(&manifest.filename);

        if !model_file.exists() {
            anyhow::bail!(
                "model file {} is missing; run `herald model download --manifest {}` first",
                model_file.display(),
                manifest_path.display()
            );
        }

        let binary = env::var_os("HERALD_LLAMA_SERVER")
            .map_or_else(|| PathBuf::from("llama-server"), PathBuf::from);

        let config = SidecarConfig::new(binary, &model_file);
        let sidecar = LlamaSidecar::start(&config)
            .await
            .map_err(|error| anyhow::anyhow!("starting inference sidecar: {error}"))?;

        Ok(Self { sidecar })
    }

    /// Tier 1: pick one operation enum. Two tokens, grammar-forced.
    pub async fn classify(&self, instruction: &str) -> Result<(Operation, bool)> {
        let user = format!(
            "Classify this request:\n<user-instruction>\n{}\n</user-instruction>",
            instruction.trim()
        );
        let schema = herald_core::intent::classifier_schema();
        let system = herald_core::intent::classifier_system();

        let raw = self
            .sidecar
            .complete_json(&system, &user, CLASSIFY_MAX_TOKENS, Some(&schema))
            .await
            .map_err(|error| anyhow::anyhow!("inference failed: {error}"))?;

        match parse_classifier(&raw) {
            Ok(result) => Ok(result),
            Err(error) => {
                let retry = self
                    .sidecar
                    .complete_json(&system, &user, RETRY_MAX_TOKENS, Some(&schema))
                    .await
                    .map_err(|retry_error| {
                        anyhow::anyhow!("inference retry failed: {retry_error}")
                    })?;
                parse_classifier(&retry).map_err(|_| error.context("model classification failed"))
            }
        }
    }

    /// Tier 3: draft a reply body. Plain text in, plain text out, no JSON.
    pub async fn draft(&self, addressed_to: &str, intent: &str) -> Result<String> {
        let user = format!(
            "<reply-to>\n{addressed_to}\n</reply-to>\n<convey>\n{}\n</convey>",
            intent.trim()
        );
        self.sidecar
            .complete_text(
                &herald_core::intent::draft_system(),
                &user,
                DRAFT_MAX_TOKENS,
            )
            .await
            .map_err(|error| anyhow::anyhow!("drafting failed: {error}"))
    }

    pub fn shutdown(&mut self) {
        self.sidecar.shutdown();
    }
}

/// Compose a full `ActionPlan` from tier 1 output plus deterministic hints.
///
/// The model chooses the operation. Target and body hints come from the
/// deterministic extractor in `herald-core::rules`; neither is treated as a
/// resolved recipient, and `validate_plan` still gates everything.
pub async fn plan_via_classifier(
    setup: &ModelSetup,
    provider: herald_core::ProviderId,
    instruction: &str,
) -> Result<ActionPlan> {
    let (operation, needs_clarification) = setup.classify(instruction).await?;

    let target = rules::target_hint(instruction);
    let reply_body = rules::body_hint(instruction);

    Ok(ActionPlan {
        provider,
        operation,
        target,
        reply_body,
        needs_clarification,
        clarification_question: needs_clarification
            .then(|| "Which message do you mean?".to_string()),
    })
}

fn parse_classifier(raw: &str) -> Result<(Operation, bool)> {
    let json = engine_json::json_slice(raw).context("model output contained no JSON object")?;
    let value: serde_json::Value =
        serde_json::from_str(json).context("model output is not valid JSON")?;
    let operation: Operation = serde_json::from_value(value["operation"].clone())
        .context("unknown operation in classifier output")?;
    let needs_clarification = value
        .get("needs_clarification")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    Ok((operation, needs_clarification))
}

mod engine_json {
    //! Brace-balanced JSON extraction, used by `parse_classifier` to ignore
    //! fences and commentary around the model's JSON object.

    /// Find the first substring that parses as a JSON object, ignoring prose.
    pub fn json_slice(raw: &str) -> Option<&str> {
        let starts: Vec<usize> = raw
            .char_indices()
            .filter_map(|(index, character)| (character == '{').then_some(index))
            .collect();

        for start in starts {
            if let Some(end) = object_end(&raw[start..]) {
                let candidate = &raw[start..start + end];
                if serde_json::from_str::<serde_json::Value>(candidate).is_ok() {
                    return Some(candidate);
                }
            }
        }

        None
    }

    /// Byte offset just past the first balanced object in `raw`.
    ///
    /// Only ever called with a slice that starts at `{`, so the depth counter
    /// cannot underflow: it starts by incrementing to 1.
    fn object_end(raw: &str) -> Option<usize> {
        let mut depth = 0usize;
        let mut in_string = false;
        let mut escaped = false;

        for (offset, character) in raw.char_indices() {
            if in_string {
                if escaped {
                    escaped = false;
                } else {
                    match character {
                        '\\' => escaped = true,
                        '"' => in_string = false,
                        _ => {}
                    }
                }
            } else {
                match character {
                    '"' => in_string = true,
                    '{' => depth += 1,
                    '}' => {
                        if depth == 1 {
                            return Some(offset + character.len_utf8());
                        }
                        depth -= 1;
                    }
                    _ => {}
                }
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::engine_json::json_slice;

    #[test]
    fn finds_object_through_fence() {
        let raw = "```json\n{\"operation\":\"send_reply\"}\n```";
        assert_eq!(json_slice(raw), Some("{\"operation\":\"send_reply\"}"));
    }

    #[test]
    fn ignores_stray_braces() {
        let raw = "here { is prose {\"a\":1} tail";
        assert_eq!(json_slice(raw), Some("{\"a\":1}"));
    }

    #[test]
    fn braces_inside_strings_do_not_close() {
        let raw = "{\"body\":\"keep } this\"}";
        assert_eq!(json_slice(raw), Some(raw));
    }

    #[test]
    fn unterminated_object_is_filtered_out() {
        assert_eq!(json_slice("{\"operation\":\"send"), None);
    }
}
