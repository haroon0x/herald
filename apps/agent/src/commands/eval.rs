//! `herald eval` — score a planning engine on the checked-in command set.
//!
//! This measures planning only. It never executes a read or a send.

use std::path::Path;

use serde::Deserialize;
use serde_json::json;

use herald_core::{ActionPlan, ProviderId, rules, validate_plan};

use crate::app::App;
use crate::cli::EngineArg;
use crate::error::AppError;
use crate::planner::{self, Model};

/// The checked-in set is embedded so `herald eval` works from an installed
/// binary. `--eval-file` still overrides it for experiments.
const DEFAULT_EVAL: &str = include_str!("../../../../eval/commands.json");

#[derive(Debug, Deserialize)]
struct EvalCase {
    id: String,
    instruction: String,
    #[serde(default)]
    operation: Option<String>,
    #[serde(default)]
    clarify: bool,
    #[serde(default)]
    body_contains: Option<String>,
    #[serde(default)]
    target_contains: Option<String>,
}

#[derive(Debug)]
enum Outcome {
    Pass(&'static str),
    Fail(String),
}

#[allow(clippy::too_many_lines)]
pub async fn run(
    app: &App,
    engine: EngineArg,
    eval_file: Option<&Path>,
    min_score: u8,
    provider_override: Option<&str>,
) -> Result<u8, AppError> {
    let provider = match provider_override {
        Some(name) => name
            .parse::<ProviderId>()
            .map_err(|_| AppError::new("unknown_provider", format!("unknown provider: {name}")))?,
        None => ProviderId::Fake,
    };

    let cases: Vec<EvalCase> = match eval_file {
        Some(path) => {
            let file = std::fs::File::open(path).map_err(|error| {
                AppError::new("eval", format!("opening {}: {error}", path.display()))
            })?;
            serde_json::from_reader(file).map_err(|error| {
                AppError::new("eval", format!("parsing {}: {error}", path.display()))
            })?
        }
        None => serde_json::from_str(DEFAULT_EVAL).map_err(|error| {
            AppError::new("eval", format!("parsing embedded eval set: {error}"))
        })?,
    };

    if cases.is_empty() {
        return Err(AppError::new("eval", "the eval set is empty"));
    }

    let model: Option<Model> =
        match planner::build_model(engine, &app.config, &app.models_dir).await {
            Ok(model) => model,
            Err(error) if engine == EngineArg::Auto => {
                app.out
                    .warn(format!("model unavailable; scoring rules only ({error})"));
                None
            }
            Err(error) => return Err(error),
        };

    let mut passes = 0usize;
    let mut rule_hits = 0usize;
    let mut results = Vec::new();

    for case in &cases {
        let outcome = match engine {
            EngineArg::Rules => score_rules(rules::plan_offline(provider, &case.instruction), case),
            EngineArg::Auto => match rules::plan_offline(provider, &case.instruction) {
                Some(plan) => {
                    rule_hits += 1;
                    score(plan, case, "rules")
                }
                None => match model.as_ref() {
                    Some(model) => match model.classify(&case.instruction).await {
                        Ok((operation, clarify)) => score(
                            plan_from_classifier(provider, &case.instruction, operation, clarify),
                            case,
                            "model",
                        ),
                        Err(error) => Outcome::Fail(format!("model failed: {error}")),
                    },
                    None => score_rules(None, case),
                },
            },
            EngineArg::Model => match model.as_ref() {
                Some(model) => match model.classify(&case.instruction).await {
                    Ok((operation, clarify)) => score(
                        plan_from_classifier(provider, &case.instruction, operation, clarify),
                        case,
                        "model",
                    ),
                    Err(error) => Outcome::Fail(format!("model failed: {error}")),
                },
                None => Outcome::Fail("no model available".into()),
            },
        };

        match outcome {
            Outcome::Pass(source) => {
                passes += 1;
                app.out.line(format!("PASS [{source}] {}", case.id));
                results.push(json!({"id": case.id, "ok": true, "source": source}));
            }
            Outcome::Fail(reason) => {
                app.out.line(format!("FAIL {}  {reason}", case.id));
                results.push(json!({"id": case.id, "ok": false, "reason": reason}));
            }
        }
    }

    let total = cases.len();
    let percentage = u8::try_from(passes.saturating_mul(100) / total).unwrap_or(100);
    app.out.blank();
    app.out.line(format!(
        "{passes}/{total} passed [{engine}], {percentage}%; rules hits: {rule_hits}"
    ));

    let below = percentage < min_score;
    if below {
        app.out
            .line(format!("below --min-score {min_score}%; exit status 5"));
    }

    app.out.data(
        "eval",
        json!({
            "engine": engine.to_string(),
            "passes": passes,
            "total": total,
            "percentage": percentage,
            "min_score": min_score,
            "below_threshold": below,
            "rules_hits": rule_hits,
            "results": results,
        }),
    );

    Ok(if below { 5 } else { 0 })
}

fn plan_from_classifier(
    provider: ProviderId,
    instruction: &str,
    operation: herald_core::Operation,
    needs_clarification: bool,
) -> ActionPlan {
    ActionPlan {
        provider,
        operation,
        target: rules::target_hint(instruction),
        reply_body: rules::body_hint(instruction),
        needs_clarification,
        clarification_question: needs_clarification.then(|| "Which message do you mean?".into()),
    }
}

fn score_rules(plan: Option<ActionPlan>, case: &EvalCase) -> Outcome {
    let Some(plan) = plan else {
        return Outcome::Fail(if case.clarify {
            "rules declined; declining is not a clarification question".into()
        } else {
            "no deterministic plan (needs the model fallback)".into()
        });
    };
    score(plan, case, "rules")
}

fn score(plan: ActionPlan, case: &EvalCase, source: &'static str) -> Outcome {
    if case.clarify {
        return if plan.needs_clarification {
            Outcome::Pass(source)
        } else {
            Outcome::Fail(format!(
                "expected a clarification, planned {}",
                plan.operation.as_str()
            ))
        };
    }
    if plan.needs_clarification {
        return Outcome::Fail("deferred when a plan was expected".into());
    }

    if let Some(expected) = case.operation.as_deref()
        && plan.operation.as_str() != expected
    {
        return Outcome::Fail(format!(
            "expected {expected}, selected {}",
            plan.operation.as_str()
        ));
    }

    if let Some(needle) = case.body_contains.as_deref() {
        let Some(body) = plan.reply_body.as_deref() else {
            return Outcome::Fail(format!("missing reply body, expected {needle:?}"));
        };
        if !body.to_lowercase().contains(&needle.to_lowercase()) {
            return Outcome::Fail(format!("{needle:?} not found in reply body"));
        }
    }

    if let Some(needle) = case.target_contains.as_deref()
        && !plan
            .target
            .describe()
            .to_lowercase()
            .contains(&needle.to_lowercase())
    {
        return Outcome::Fail(format!(
            "target {:?} does not contain {needle:?}",
            plan.target.describe()
        ));
    }

    match validate_plan(plan) {
        Ok(_) => Outcome::Pass(source),
        Err(error) => Outcome::Fail(format!("plan did not validate: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use herald_core::{Operation, Target};

    fn case(id: &str, operation: Option<&str>, clarify: bool) -> EvalCase {
        EvalCase {
            id: id.into(),
            instruction: "irrelevant".into(),
            operation: operation.map(str::to_string),
            clarify,
            body_contains: None,
            target_contains: None,
        }
    }

    fn plan(operation: Operation, clarify: bool) -> ActionPlan {
        ActionPlan {
            provider: ProviderId::Fake,
            operation,
            target: Target::Description {
                description: "the latest message".into(),
            },
            reply_body: None,
            needs_clarification: clarify,
            clarification_question: None,
        }
    }

    #[test]
    fn a_clarification_passes_a_clarify_case() {
        assert!(matches!(
            score(
                plan(Operation::ListUnreadMessages, true),
                &case("a", None, true),
                "model"
            ),
            Outcome::Pass("model")
        ));
    }

    #[test]
    fn declining_is_not_clarifying() {
        assert!(matches!(
            score_rules(None, &case("a", None, true)),
            Outcome::Fail(_)
        ));
    }

    #[test]
    fn a_plan_passes_a_matching_case() {
        assert!(matches!(
            score(
                plan(Operation::ListUnreadMessages, false),
                &case("a", Some("list_unread_messages"), false),
                "rules"
            ),
            Outcome::Pass("rules")
        ));
    }

    #[test]
    fn a_wrong_operation_fails() {
        assert!(matches!(
            score(
                plan(Operation::ListRecentMessages, false),
                &case("a", Some("list_unread_messages"), false),
                "rules"
            ),
            Outcome::Fail(_)
        ));
    }
}
