use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use directories::ProjectDirs;

use herald_core::{
    Action, ActionState, MessageQuery, PolicyConfig, ProviderId, authorize, rules, time::now_ms,
    validate_plan,
};
use herald_provider::fake::FakeProvider;
use herald_provider::{CommunicationProvider, ProviderHealth};
use herald_storage::{AuditEvent, Store, new_action};

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

    match cli.command {
        Commands::Doctor => doctor(&data_dir, &db_path),
        Commands::Plan {
            instruction,
            provider,
            enable_send,
        } => plan_command(&db_path, &instruction, &provider, enable_send),
        Commands::Providers => providers().await,
        Commands::Inbox { unread, limit } => inbox(unread, limit).await,
        Commands::Thread { id } => thread(&id).await,
        Commands::History { limit } => history(&db_path, limit),
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

fn plan_command(
    db_path: &std::path::Path,
    instruction: &str,
    provider: &str,
    enable_send: bool,
) -> Result<()> {
    let provider = ProviderId::from_str(provider).map_err(anyhow::Error::msg)?;

    let Some(raw_plan) = rules::plan_offline(provider, instruction) else {
        println!("No deterministic plan for this instruction.");
        println!("The local-model fallback is not wired yet (next milestone).");
        return Ok(());
    };

    println!("provider:  {provider}");
    println!("operation: {:?}", raw_plan.operation);
    println!("target:    {}", raw_plan.target.describe());

    let action = match validate_plan(raw_plan) {
        Ok(action) => action,
        Err(error) => {
            println!("decision:  clarification required");
            println!("reason:    {error}");
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

    Ok(())
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
