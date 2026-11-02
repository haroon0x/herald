use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use directories::ProjectDirs;

use herald_core::{
    Action, ActionPlan, ActionState, MessageQuery, Operation, PolicyConfig, ProviderId, authorize,
    rules, time::now_ms, validate_plan,
};
use herald_provider::fake::FakeProvider;
use herald_provider::{CommunicationProvider, ProviderHealth};
use herald_storage::{AuditEvent, Store, new_action};

mod engine;

use crate::engine::{Engine, ModelSetup, plan_via_classifier};

#[derive(Debug, Parser)]
#[command(
    name = "herald",
    version,
    about = "Local-first communication agent",
    long_about = "Herald reads connected communication providers, summarizes and drafts locally, \
                  and only sends after an explicit policy check and approval."
)]
struct Cli {
    /// Override the data directory (defaults to the platform data dir).
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Check local state, storage, and inference configuration.
    Doctor,
    /// Turn an instruction into a validated plan and show the policy decision.
    Plan {
        instruction: String,
        #[arg(long, default_value = "gmail")]
        provider: String,
        /// rules | auto (rules first, model fallback) | model
        #[arg(long, default_value = "rules")]
        engine: String,
        #[arg(long)]
        manifest: Option<PathBuf>,
        /// Enable outbound actions so the approval path can be inspected.
        #[arg(long)]
        enable_send: bool,
    },
    /// List known providers and their health.
    Providers,
    /// Read recent messages from the fake provider.
    Inbox {
        #[arg(long)]
        unread: bool,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// Print a thread from the fake provider.
    Thread { id: String },
    /// Show recorded action history.
    History {
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Download or verify the pinned model artifact.
    Model {
        #[command(subcommand)]
        action: ModelAction,
    },
    /// Score the command set against an engine (Phase 0 eval harness).
    Eval {
        /// rules | model
        #[arg(long, default_value = "rules")]
        engine: String,
        #[arg(long)]
        manifest: Option<PathBuf>,
        #[arg(long)]
        eval_file: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum ModelAction {
    Download {
        #[arg(long)]
        manifest: Option<PathBuf>,
    },
    Verify {
        #[arg(long)]
        manifest: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let data_dir = match cli.data_dir {
        Some(path) => path,
        None => default_data_dir()?,
    };
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("creating data directory {}", data_dir.display()))?;
    let db_path = data_dir.join("herald.db");
    let models_dir = data_dir.join("models");

    match cli.command {
        Commands::Doctor => doctor(&data_dir, &db_path),
        Commands::Plan {
            instruction,
            provider,
            engine,
            manifest,
            enable_send,
        } => {
            plan_command(
                &db_path,
                &instruction,
                &provider,
                &engine,
                manifest.as_deref(),
                &models_dir,
                enable_send,
            )
            .await
        }
        Commands::Providers => providers().await,
        Commands::Inbox { unread, limit } => inbox(unread, limit).await,
        Commands::Thread { id } => thread(&id).await,
        Commands::History { limit } => history(&db_path, limit),
        Commands::Model { action } => match action {
            ModelAction::Download { manifest } => {
                model_command(true, manifest.as_deref(), &models_dir).await
            }
            ModelAction::Verify { manifest } => {
                model_command(false, manifest.as_deref(), &models_dir).await
            }
        },
        Commands::Eval {
            engine,
            manifest,
            eval_file,
        } => {
            eval_command(
                &engine,
                manifest.as_deref(),
                eval_file.as_deref(),
                &models_dir,
            )
            .await
        }
    }
}

fn default_data_dir() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("HERALD_DATA_DIR") {
        return Ok(PathBuf::from(path));
    }
    ProjectDirs::from("dev", "Herald", "herald")
        .map(|dirs| dirs.data_dir().to_path_buf())
        .context("could not determine a platform data directory")
}

fn resolve_engine(name: &str) -> Result<Engine> {
    match name {
        "rules" => Ok(Engine::Rules),
        "auto" => Ok(Engine::Auto),
        "model" => Ok(Engine::Model),
        other => Err(anyhow::anyhow!(
            "unknown engine {other}; expected rules, auto, or model"
        )),
    }
}

fn doctor(data_dir: &std::path::Path, db_path: &std::path::Path) -> Result<()> {
    println!("Herald {}", env!("CARGO_PKG_VERSION"));
    println!("data dir:  {}", data_dir.display());
    println!("database:  {}", db_path.display());

    let store = Store::open(db_path).context("opening local database")?;
    println!("messages:  {}", store.message_count()?);
    println!("actions:   {}", store.recent_actions(1_000)?.len());
    println!("audit:     {}", store.audit_count()?);

    let manifest = data_dir.join("models").join("manifest.json");
    if manifest.exists() {
        let manifest = herald_runtime::ModelManifest::load(&manifest)?;
        println!(
            "model:     {} ({}, {})",
            manifest.name, manifest.quantization, manifest.license
        );
    } else {
        println!("model:     not configured (download lands in Phase 1)");
    }

    match std::env::var_os("HERALD_LLAMA_SERVER") {
        Some(path) => println!("llama:     {}", PathBuf::from(path).display()),
        None => println!("llama:     HERALD_LLAMA_SERVER is not set"),
    }

    Ok(())
}

async fn maybe_setup_model(
    engine: Engine,
    manifest: Option<&std::path::Path>,
    models_dir: &std::path::Path,
) -> Result<Option<ModelSetup>> {
    match engine {
        Engine::Rules => Ok(None),
        Engine::Auto | Engine::Model => {
            let manifest = match manifest {
                Some(path) => path.to_path_buf(),
                None => models_dir.join("manifest.json"),
            };
            match ModelSetup::start(&manifest, models_dir).await {
                Ok(setup) => Ok(Some(setup)),
                Err(error) => {
                    if engine == Engine::Model {
                        return Err(error);
                    }
                    println!("note:      model fallback unavailable ({error}); using rules only");
                    Ok(None)
                }
            }
        }
    }
}

async fn plan_command(
    db_path: &std::path::Path,
    instruction: &str,
    provider: &str,
    engine_name: &str,
    manifest: Option<&std::path::Path>,
    models_dir: &std::path::Path,
    enable_send: bool,
) -> Result<()> {
    let provider = ProviderId::from_str(provider).map_err(anyhow::Error::msg)?;
    let engine = resolve_engine(engine_name)?;
    let mut setup = maybe_setup_model(engine, manifest, models_dir).await?;

    let Some(mut raw_plan) = route_plan(engine, &mut setup, provider, instruction).await else {
        println!("No plan for this instruction.");
        if matches!(engine, Engine::Rules) {
            println!("The local-model fallback is not enabled for this run.");
        }
        setup.as_mut().map(ModelSetup::shutdown);
        return Ok(());
    };

    if let Some(setup) = setup.as_mut() {
        draft_missing_body(setup, &mut raw_plan, instruction).await;
    }

    println!("provider:  {provider}");
    println!("operation: {:?}", raw_plan.operation);
    println!("target:    {}", raw_plan.target.describe());

    let action = match validate_plan(raw_plan) {
        Ok(action) => action,
        Err(error) => {
            println!("decision:  clarification required");
            println!("reason:    {error}");
            setup.as_mut().map(ModelSetup::shutdown);
            return Ok(());
        }
    };

    if let Action::Draft(draft) = &action {
        println!("body:      {}", draft.body);
    }
    if let Action::Send(send) = &action {
        println!("body:      {}", send.body);
        println!("hash:      {}", send.body_hash);
    }

    let policy = PolicyConfig {
        allow_send: enable_send,
        ..PolicyConfig::default()
    };
    let decision = authorize(&action, &policy);

    let store = Store::open(db_path)?;
    let action_id = uuid::Uuid::new_v4().to_string();
    let record = new_action(
        &action_id,
        action.provider(),
        raw_operation(&action),
        action.target().describe(),
        content_hash(&action),
    );
    store.create_action(&record)?;

    match &decision {
        herald_core::Decision::Allowed => {
            println!("decision:  allowed (no account mutation)");
            println!("note:      plan only; provider execution lands in Phase 2");
            store.append_audit(&AuditEvent {
                action_id: Some(action_id),
                at_ms: now_ms(),
                status: "allowed".into(),
                detail: "read/draft is local".into(),
            })?;
        }
        herald_core::Decision::NeedsApproval(request) => {
            store.transition_action(&action_id, ActionState::AwaitingApproval)?;
            println!("decision:  awaiting approval");
            println!(
                "approve:   {} -> {}",
                request.provider,
                request.target.describe()
            );
            println!("body:      {}", request.body);
            store.append_audit(&AuditEvent {
                action_id: Some(action_id),
                at_ms: now_ms(),
                status: "awaiting_approval".into(),
                detail: "send requires explicit confirmation".into(),
            })?;
            println!("\nNot sent. Provider execution lands in Phase 3.");
        }
        herald_core::Decision::Rejected(reason) => {
            store.transition_action(&action_id, ActionState::Failed)?;
            println!("decision:  rejected");
            println!("reason:    {reason}");
            store.append_audit(&AuditEvent {
                action_id: Some(action_id),
                at_ms: now_ms(),
                status: "rejected".into(),
                detail: reason.to_string(),
            })?;
        }
    }

    setup.as_mut().map(ModelSetup::shutdown);
    Ok(())
}

/// Deterministic routing first; the model classifier covers what rules cannot.
///
/// `Engine::Rules` never reaches the model; `Auto` and `Model` fall back to the
/// tier-1 classifier when the rules router has no answer.
async fn route_plan(
    engine: Engine,
    setup: &mut Option<ModelSetup>,
    provider: ProviderId,
    instruction: &str,
) -> Option<ActionPlan> {
    if let Some(plan) = rules::plan_offline(provider, instruction) {
        return Some(plan);
    }

    if matches!(engine, Engine::Rules) {
        return None;
    }

    match setup.as_mut() {
        Some(setup) => match plan_via_classifier(setup, provider, instruction).await {
            Ok(plan) => Some(plan),
            Err(error) => {
                println!("note:      model planning failed ({error})");
                None
            }
        },
        None => None,
    }
}

/// Fill in the body of an unquoted reply plan with the tier-3 draft model.
///
/// Plain text in, plain text out. The draft still passes through
/// `validate_plan` and the policy check afterwards like every other plan.
async fn draft_missing_body(setup: &mut ModelSetup, plan: &mut ActionPlan, instruction: &str) {
    if plan.operation != Operation::PrepareReply || plan.reply_body.is_some() {
        return;
    }

    match setup.draft(&plan.target.describe(), instruction).await {
        Ok(body) if !body.trim().is_empty() => {
            plan.reply_body = Some(body.trim().to_string());
        }
        Ok(_) => println!("note:      the model returned an empty draft; plan stays bodyless"),
        Err(error) => println!("note:      drafting failed ({error}); plan stays bodyless"),
    }
}

fn raw_operation(action: &Action) -> &'static str {
    match action {
        Action::Read(read) => match read.operation {
            herald_core::ReadOperation::ListRecentMessages => "list_recent_messages",
            herald_core::ReadOperation::ListUnreadMessages => "list_unread_messages",
            herald_core::ReadOperation::GetThread => "get_thread",
            herald_core::ReadOperation::SearchMessages => "search_messages",
            herald_core::ReadOperation::SummarizeThread => "summarize_thread",
        },
        Action::Draft(_) => "prepare_reply",
        Action::Send(_) => "send_reply",
    }
}

fn content_hash(action: &Action) -> Option<String> {
    match action {
        Action::Send(send) => Some(send.body_hash.clone()),
        Action::Draft(_) | Action::Read(_) => None,
    }
}

async fn providers() -> Result<()> {
    let provider = FakeProvider::new();
    let health = provider.health().await;
    let label = match health {
        ProviderHealth::Ready => "ready",
        ProviderHealth::NotAuthenticated => "not authenticated",
        ProviderHealth::Unavailable(_) => "unavailable",
    };
    println!("fake      {label}  (in-memory, deterministic)");
    println!("gmail     not implemented");
    println!("x         not implemented");
    Ok(())
}

async fn inbox(unread: bool, limit: usize) -> Result<()> {
    let provider = FakeProvider::new();
    let query = if unread {
        MessageQuery::unread(limit)
    } else {
        MessageQuery::recent(limit)
    };
    let messages = provider.list_recent(&query).await?;

    if messages.is_empty() {
        println!("No messages.");
        return Ok(());
    }

    for message in messages {
        let flag = if message.unread { "*" } else { " " };
        println!(
            "{flag} {:<32} {:<40} [{}]",
            message.sender, message.subject, message.thread_id
        );
        println!("  id: {}", message.id);
    }
    Ok(())
}

async fn thread(id: &str) -> Result<()> {
    let provider = FakeProvider::new();
    let thread = provider.get_thread(&herald_core::ThreadId::new(id)).await?;
    println!("{} — {} message(s)", thread.subject, thread.messages.len());
    for message in thread.messages {
        println!("\n--- {} ---", message.sender);
        let body: String = message.body.as_ref().chars().take(500).collect();
        println!("{body}");
    }
    Ok(())
}

fn history(db_path: &std::path::Path, limit: usize) -> Result<()> {
    let store = Store::open(db_path)?;
    let actions = store.recent_actions(limit)?;

    if actions.is_empty() {
        println!("No recorded actions.");
        return Ok(());
    }

    for action in actions {
        println!(
            "{}  {:<24} {:<20} {:?}  {}",
            action.action_id, action.operation, action.target, action.state, action.provider
        );
    }
    Ok(())
}

async fn model_command(
    download: bool,
    manifest: Option<&std::path::Path>,
    models_dir: &std::path::Path,
) -> Result<()> {
    let manifest_path = match manifest {
        Some(path) => path.to_path_buf(),
        None => models_dir.join("manifest.json"),
    };
    let manifest = herald_runtime::ModelManifest::load(&manifest_path)
        .with_context(|| format!("loading manifest {}", manifest_path.display()))?;

    if download {
        let url = std::env::var("HERALD_MODEL_URL")
            .ok()
            .filter(|value| !value.trim().is_empty());
        match herald_runtime::download_model(&manifest, models_dir, url).await {
            Ok(None) => println!("verified {} (already present)", manifest.filename),
            Ok(Some(stats)) => println!(
                "downloaded {} — {} bytes in {} ms (resumed: {})",
                manifest.filename, stats.bytes_written, stats.elapsed_ms, stats.resumed
            ),
            Err(error) => anyhow::bail!("download failed: {error}"),
        }
    } else {
        manifest.verify(models_dir.join(&manifest.filename))?;
        println!("checksum ok: {}", manifest.filename);
    }

    Ok(())
}

#[derive(Debug, serde::Deserialize)]
struct EvalCase {
    id: String,
    instruction: String,
    #[serde(default)]
    operation: Option<String>,
    #[serde(default = "default_false")]
    clarify: bool,
    #[serde(default)]
    body_contains: Option<String>,
}

fn default_false() -> bool {
    false
}

enum EvalOutcome {
    Pass(String),
    Fail(String),
}

/// The checked-in eval set, resolved from the repo root at test-run time.
fn default_eval_file() -> Result<std::path::PathBuf> {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir
        .parent()
        .and_then(std::path::Path::parent)
        .map(std::path::Path::to_path_buf)
        .context("resolving repo root")?;
    Ok(repo_root.join("eval").join("commands.json"))
}

async fn eval_command(
    engine_name: &str,
    manifest: Option<&std::path::Path>,
    eval_file: Option<&std::path::Path>,
    models_dir: &std::path::Path,
) -> Result<()> {
    let engine = resolve_engine(engine_name)?;
    let eval_file = match eval_file {
        Some(path) => Ok(path.to_path_buf()),
        None => default_eval_file(),
    }?;

    let file = std::fs::File::open(&eval_file)
        .with_context(|| format!("opening eval set {}", eval_file.display()))?;
    let cases: Vec<EvalCase> =
        serde_json::from_reader(file).context("parsing eval commands file")?;

    let mut setup = maybe_setup_model(engine, manifest, models_dir).await?;
    let provider = ProviderId::Gmail;

    let mut passes: usize = 0;
    let mut rule_passes: usize = 0;

    for case in &cases {
        let outcome = match engine {
            Engine::Rules => {
                let plan = rules::plan_offline(provider, &case.instruction);
                score_rules(plan, case)
            }
            Engine::Auto => match rules::plan_offline(provider, &case.instruction) {
                Some(plan) => {
                    rule_passes += 1;
                    score_model_or_rules(Ok(plan), case)
                }
                None => match setup.as_mut() {
                    Some(setup) => score_model_or_rules(
                        plan_via_classifier(setup, provider, &case.instruction).await,
                        case,
                    ),
                    None => score_rules(None, case),
                },
            },
            Engine::Model => {
                let result = match setup.as_mut() {
                    Some(setup) => plan_via_classifier(setup, provider, &case.instruction).await,
                    None => Err(anyhow::anyhow!("no model configured for --engine model")),
                };
                score_model_or_rules(result, case)
            }
        };

        match outcome {
            EvalOutcome::Pass(who) => {
                passes += 1;
                println!("PASS [{who}] {}", case.id);
            }
            EvalOutcome::Fail(reason) => {
                println!("FAIL {}  {reason}", case.id);
            }
        }
    }

    let total = cases.len();
    println!("\n{passes}/{total} passed [{engine_name} engine]; rules strength: {rule_passes}");
    setup.as_mut().map(ModelSetup::shutdown);

    if passes < total {
        std::process::exit(1);
    }

    Ok(())
}

fn score_rules(plan: Option<herald_core::ActionPlan>, case: &EvalCase) -> EvalOutcome {
    if case.clarify {
        return if plan.is_none() {
            EvalOutcome::Pass("rules".into())
        } else {
            EvalOutcome::Fail("expected deferral to clarification, got a plan".into())
        };
    }

    let Some(plan) = plan else {
        return EvalOutcome::Fail("no deterministic plan (needs the model fallback)".into());
    };
    score_plan(plan, case, "rules")
}

fn score_model_or_rules(
    result: Result<herald_core::ActionPlan, anyhow::Error>,
    case: &EvalCase,
) -> EvalOutcome {
    if case.clarify {
        return if result.is_err() {
            EvalOutcome::Pass("model".into())
        } else {
            EvalOutcome::Fail("expected clarification, got a plan".into())
        };
    }

    match result {
        Ok(plan) => score_plan(plan, case, "model"),
        Err(error) => EvalOutcome::Fail(format!("model failed: {error}")),
    }
}

fn score_plan(plan: herald_core::ActionPlan, case: &EvalCase, who: &str) -> EvalOutcome {
    if case.clarify {
        return if plan.needs_clarification {
            EvalOutcome::Pass(who.into())
        } else {
            EvalOutcome::Fail(format!(
                "expected deferral, model planned {}",
                plan.operation.as_str()
            ))
        };
    }
    if plan.needs_clarification {
        return EvalOutcome::Fail("deferred when a plan was expected".into());
    }

    let actual = plan.operation;
    let Some(expected) = case.operation.as_deref() else {
        // No expectation beyond "some plan"; structural validity is enough.
        return match validate_plan(plan) {
            Ok(_) => EvalOutcome::Pass(who.into()),
            Err(error) => EvalOutcome::Fail(format!("plan did not validate: {error}")),
        };
    };

    if actual.as_str() != expected {
        return EvalOutcome::Fail(format!("expected {expected}, selected {}", actual.as_str()));
    }

    if let Some(needle) = case.body_contains.as_deref() {
        let Some(body) = plan.reply_body.as_deref() else {
            return EvalOutcome::Fail(format!(
                "missing reply body, expected it to contain {needle:?}"
            ));
        };
        if !body.contains(needle) {
            return EvalOutcome::Fail(format!("{needle:?} not found in {body:?}"));
        }
    }

    match validate_plan(plan) {
        Ok(_) => EvalOutcome::Pass(who.into()),
        Err(error) => EvalOutcome::Fail(format!("plan did not validate: {error}")),
    }
}
