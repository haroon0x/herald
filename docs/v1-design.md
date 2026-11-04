# Herald v1 design (working spec)

This is the contract for the v1.0 build. It is intentionally concrete: the
repository must ship a working local-first agent, not a scaffold.

## Product

`herald` is a single local binary that turns a plain-language request into a
typed action against the user's own communications, and never sends anything
without an explicit, inspectable approval.

v1 ships two providers:

| provider | read | write | purpose |
| --- | --- | --- | --- |
| `fake` | seeded demo inbox | in-memory send, verified | tests, demos, eval |
| `maildir` | local Maildir (`new/`, `cur/`) | SMTP + copy to `Sent` | real local mail |

`gmail` and `x` remain enum variants marked **planned**; no adapter exists and
the CLI must say so rather than pretend.

## Non-negotiable invariants

1. The model never executes. It emits a plan; Rust validates, resolves,
   authorizes, and executes.
2. Message content is untrusted data. Prompts wrap it; tests prove it.
3. Every write is idempotent and passes through
   `planned -> awaiting_approval -> executing -> succeeded | failed | uncertain`
   (plus `cancelled`). `uncertain` is reconciled, never blind-retried.
4. Sending requires approval every time. No auto-send, including demo mode.
5. Inference is local by default. No content leaves the machine unless the user
   configures an endpoint explicitly.
6. Surprises stop and report. No captcha/login/DOM workarounds; Maildir has no
   login, SMTP failures are reported as failures.

## Data flow

```text
instruction
  -> rules router (deterministic)
  -> classifier (local model, grammar-constrained) if rules miss
  -> ActionPlan JSON (deny_unknown_fields, closed enums)
  -> validate_plan -> Action
  -> authorize -> allowed | needs_approval | rejected
  -> target resolution (provider data, fail closed)
  -> approval row (body + hash + resolved thread) when outbound
  -> approve -> re-resolve -> prepare -> send -> verify -> receipt
  -> SQLite actions + audit (no full bodies retained after decision)
```

## Core types

`herald-core` additions:

- `ProviderId::Maildir`.
- `ActionState::Cancelled` (terminal). Transitions:
  `Planned -> AwaitingApproval | Executing | Failed | Cancelled`,
  `AwaitingApproval -> Executing | Failed | Cancelled`,
  `Executing -> Succeeded | Failed | Uncertain`,
  `Uncertain -> Succeeded | Failed`.
- `PolicyConfig` unchanged in shape (`allow_send`, `max_body_chars`), but the
  default stays `allow_send = false`.

## Provider port

```rust
#[async_trait]
pub trait CommunicationProvider: Send + Sync {
    fn id(&self) -> ProviderId;
    async fn health(&self) -> ProviderHealth;
    async fn list_recent(&self, query: &MessageQuery) -> Result<Vec<MessageSummary>, ProviderError>;
    async fn get_thread(&self, id: &ThreadId) -> Result<Thread, ProviderError>;
    async fn prepare_reply(&self, id: &ThreadId, body: &str, client_token: &str)
        -> Result<PreparedAction, ProviderError>;
    async fn send_reply(&self, action: &PreparedAction) -> Result<ExecutionReceipt, ProviderError>;
    /// Look for a message this client already sent, to resolve `uncertain`.
    async fn find_sent(&self, thread: &ThreadId, client_token: &str)
        -> Result<Option<ExecutionReceipt>, ProviderError>;
}
```

`PreparedAction` carries `client_token` (the action id). `send_reply` must be
idempotent on that token: a second call returns the existing receipt instead of
sending again. `ProviderError::NotConfigured(String)` is added for a read-only
provider with no SMTP configuration.

## Storage

Schema version 3 adds the `pending_approvals` table and two nullable columns
on `actions` (`target_json`, `thread_id`) so a target can be re-resolved and a
timeout can be reconciled. The migration is idempotent and keeps existing rows.

```sql
CREATE TABLE IF NOT EXISTS pending_approvals (
    action_id  TEXT PRIMARY KEY,
    provider   TEXT NOT NULL,
    target     TEXT NOT NULL,        -- serialized Target JSON
    thread_id  TEXT NOT NULL,        -- resolved, re-verified before send
    body       TEXT NOT NULL,        -- deleted on decision/expiry
    body_hash  TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);
```

API: `create_approval`, `get_approval`, `pending_approvals`, `delete_approval`,
`purge_expired_approvals`, `get_action`, `recent_audit`, `cancel_action`.
`StoredMessage` conversion must take the provider from the caller, never
hardcode `Fake`.

## Configuration

TOML at `$HERALD_CONFIG`, else the platform config dir (`config.toml`). Data at
`$HERALD_DATA_DIR`, else the platform data dir. CLI flags > env > file > default.

```toml
default_provider = "fake"        # fake | maildir

[policy]
allow_send = false
max_body_chars = 8000
approval_ttl_hours = 72

[maildir]
path = "~/Mail"
sent_path = "~/.Sent"            # default: <path>/.Sent
from = "Me <me@example.com>"

[maildir.smtp]                   # optional; without it maildir is read-only
host = "smtp.example.com"
port = 587
username = "me@example.com"
password_env = "HERALD_SMTP_PASSWORD"
security = "starttls"            # starttls | tls | none

[model]
backend = "none"                 # none | managed | openai
endpoint = "http://127.0.0.1:11434/v1"
model_name = "qwen2.5:1.5b-instruct"
manifest = "manifest.json"
```

Secrets are read from the environment; a plaintext `password` is accepted but
`doctor` warns and `init` never writes one.

## Inference backends

`herald-runtime` exposes an `Inference` trait (`complete_json`, `complete_text`).
Two implementations: the managed loopback `llama-server` sidecar and a generic
OpenAI-compatible client (for a local Ollama/LM Studio/vLLM). This keeps the
model tier testable against a mock HTTP server and usable without a bundled
binary. Managed inference is still the default recommendation.

## CLI

```
herald init [--force]
herald doctor [--json]
herald ask "<instruction>" [--provider P] [--engine rules|auto|model]
                            [--enable-send] [--thread ID] [--json]
herald plan "<instruction>" ...        # dry run: no execution, no writes
herald inbox [--unread] [--limit N] [--json]
herald thread <id> [--json]
herald approvals [--json]
herald approve <action-id> [--json]
herald reject <action-id> [--json]
herald reconcile <action-id> [--json]
herald history [--limit N] [--json]
herald audit [--limit N] [--json]
herald providers [--json]
herald config get <key> | config list | config path [--json]
herald model download|verify|status|pin <file> [--json]
herald eval [--engine ...] [--json] [--min-score N]
herald completions <shell>
```

Behaviour:

- `ask` executes allowed reads/drafts; for sends it creates an approval and
  prints the exact provider, resolved thread, body, and hash. It never sends.
- `plan` only prints the validation and policy decision.
- `approve` re-resolves the target, refuses if it changed or the approval
  expired, verifies the body hash, sends, verifies, and records a receipt.
  Timeout -> `uncertain`. Other errors -> `failed`.
- `reconcile` resolves `uncertain` via `find_sent`; it never retries a send.
- `reject` cancels the action and deletes the retained body.
- `--json` emits one object on stdout: `{"ok":true,"command":"...","data":...}`;
  failures emit `{"ok":false,"error":{"code":"...","message":"..."}}`.
- Human output is plain text, colorized only on a TTY, and never prints a
  secret.

## Quality gates

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace` (unit + integration; includes injection and
  approval-bypass attempts)
- CI on Linux/macOS/Windows, MSRV 1.98, `cargo-deny`, `cargo-audit`.
- README, architecture, security, privacy, contributing, code of conduct,
  changelog, issue/PR templates, release workflow, license (AGPL-3.0).

## Deliberately out of scope for v1

IMAP/Gmail adapters, browser automation, auto-send, multi-user, sync, mobile,
notifications, and "agent loops" with tool registries. The roadmap records them.
