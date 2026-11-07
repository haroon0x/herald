//! Command-line surface.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use clap_complete::Shell;

/// Local-first communication agent.
#[derive(Debug, Parser)]
#[command(
    name = "herald",
    version,
    about = "Read local mail, draft with a local model, send only after approval",
    long_about = "Herald is a local-first communication agent. It reads connected \
                  providers, summarizes and drafts with a local model, and only sends \
                  after an explicit policy check and a human approval.",
    arg_required_else_help = true,
    after_help = "Examples:\n  \
                  herald ask \"check what is unread\"\n  \
                  herald ask 'send exactly: \"Thanks, I will review this.\"' --enable-send\n  \
                  herald approvals\n  \
                  herald approve <action-id>\n\n\
                  Docs: https://github.com/haroon0x/herald"
)]
pub struct Cli {
    /// Override the data directory (defaults to the platform data dir).
    #[arg(long, global = true)]
    pub data_dir: Option<PathBuf>,

    /// Path to config.toml (defaults to the platform config dir).
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,

    /// Emit one JSON object on stdout.
    #[arg(long, global = true)]
    pub json: bool,

    /// Disable ANSI colors. `NO_COLOR` is also honored.
    #[arg(long, global = true)]
    pub no_color: bool,

    /// Log filter, e.g. `info` or `herald_agent=debug` (default: `HERALD_LOG` or `warn`).
    #[arg(long, global = true)]
    pub log: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create the data directory, database, and a starter config.
    Init {
        /// Overwrite an existing config file.
        #[arg(long)]
        force: bool,
    },
    /// Check storage, configuration, providers, and inference.
    Doctor,
    /// Plan and execute a request. Sends stop at an approval.
    Ask {
        instruction: String,
        /// Provider override: fake, maildir, gmail, x.
        #[arg(long)]
        provider: Option<String>,
        /// rules (deterministic only), auto (rules then model), model.
        #[arg(long, value_enum, default_value_t = EngineArg::Auto)]
        engine: EngineArg,
        /// Enable the approval path for outbound actions (still requires approve).
        #[arg(long)]
        enable_send: bool,
        /// Target this exact thread id instead of resolving a phrase.
        #[arg(long)]
        thread: Option<String>,
        /// Read the exact outbound body from this environment variable instead of the model.
        /// For callers that already hold the body (the UI); empty or unset is an error.
        #[arg(long, value_name = "VAR")]
        body_env: Option<String>,
    },
    /// Dry run: show the validated plan and policy decision. No writes.
    Plan {
        instruction: String,
        #[arg(long)]
        provider: Option<String>,
        #[arg(long, value_enum, default_value_t = EngineArg::Auto)]
        engine: EngineArg,
        /// Show what the policy would decide if sending were enabled.
        #[arg(long)]
        enable_send: bool,
        /// Target this exact thread id instead of resolving a phrase.
        #[arg(long)]
        thread: Option<String>,
    },
    /// List recent messages from a provider.
    Inbox {
        #[arg(long, value_enum, default_value_t = ScopeArg::All)]
        scope: ScopeArg,
        #[arg(long)]
        unread: bool,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        #[arg(long)]
        provider: Option<String>,
    },
    /// Print one thread.
    Thread {
        id: String,
        #[arg(long)]
        provider: Option<String>,
    },
    /// List outbound actions waiting for a decision.
    Approvals,
    /// Approve one pending action and send it.
    Approve { id: String },
    /// Reject one pending action. Nothing is sent.
    Reject { id: String },
    /// Resolve an uncertain action by re-reading the provider. Never retries.
    Reconcile { id: String },
    /// Show recorded actions.
    History {
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Show the append-only audit log.
    Audit {
        #[arg(long, default_value_t = 30)]
        limit: usize,
    },
    /// List providers and their health.
    Providers,
    /// X adapter commands.
    X {
        #[command(subcommand)]
        action: XAction,
    },
    /// Read configuration values.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Manage the model artifact.
    Model {
        #[command(subcommand)]
        action: ModelAction,
    },
    /// Score the checked-in command set against an engine.
    Eval {
        #[arg(long, value_enum, default_value_t = EngineArg::Rules)]
        engine: EngineArg,
        #[arg(long)]
        eval_file: Option<PathBuf>,
        /// Fail below this score (0-100). Default: 0.
        #[arg(long, default_value_t = 0)]
        min_score: u8,
        #[arg(long)]
        provider: Option<String>,
    },
    /// Print shell completions.
    Completions { shell: Shell },
    /// Serve a local browser UI on 127.0.0.1.
    Ui {
        /// Port to bind. Use 0 for a free port.
        #[arg(long, default_value_t = 7878)]
        port: u16,
        /// Open the printed URL in the default browser (xdg-open). Failures are ignored.
        #[arg(long)]
        open: bool,
    },
}

/// Which read surface `herald inbox` asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ScopeArg {
    All,
    Feed,
    Mentions,
    Dms,
    Notifications,
}

impl From<ScopeArg> for herald_core::MessageScope {
    fn from(value: ScopeArg) -> Self {
        match value {
            ScopeArg::All => Self::All,
            ScopeArg::Feed => Self::Feed,
            ScopeArg::Mentions => Self::Mentions,
            ScopeArg::Dms => Self::Dms,
            ScopeArg::Notifications => Self::Notifications,
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum XAction {
    /// Open a visible Chrome window on the dedicated profile and wait for the
    /// human to sign in. Herald never reads cookies or types credentials.
    Login,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum EngineArg {
    Rules,
    Auto,
    Model,
}

impl std::fmt::Display for EngineArg {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Rules => "rules",
            Self::Auto => "auto",
            Self::Model => "model",
        })
    }
}

#[derive(Debug, Subcommand)]
pub enum ConfigAction {
    /// Print one dotted key.
    Get { key: String },
    /// Print every non-secret key.
    List,
    /// Print the config path in use.
    Path,
    /// Write the annotated starter config (refuses to overwrite).
    Init {
        #[arg(long)]
        force: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum ModelAction {
    /// Download and verify the manifest's model.
    Download,
    /// Verify an existing model against the manifest.
    Verify,
    /// Show the configured backend and artifact state.
    Status,
    /// Pin an existing GGUF file: compute its SHA-256 and write a manifest.
    Pin {
        file: PathBuf,
        /// Model name recorded in the manifest.
        #[arg(long, default_value = "local-model")]
        name: String,
        /// Upstream repository, for provenance.
        #[arg(long, default_value = "local")]
        repo: String,
        /// Upstream revision/commit.
        #[arg(long, default_value = "local")]
        revision: String,
        /// Quantization label.
        #[arg(long, default_value = "unknown")]
        quantization: String,
        /// Where to write the manifest (defaults to `<data-dir>/models/manifest.json`).
        #[arg(long)]
        manifest: Option<PathBuf>,
    },
}
