# Changelog

All notable changes to Herald are documented in this file.

The format is based on [Keep a Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning 2.0.0](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

In progress, not yet live-canaried. Treat these as implemented and unit-tested,
not as released behaviour until the canary in the milestone handoff passes.

### Added

- **`herald ui`**: a loopback browser UI over the same CLI and approval queue.
  Per-run token, Origin check, CSP, no inline script, explicit endpoints, one
  command at a time. See [ADR 0010](docs/decisions/0010-local-web-ui.md).

- **X provider** (`x`): reads mentions and DMs, replies, sends DMs, and creates
  original posts through a dedicated Chrome profile driven by `chromiumoxide`.
  Constant selectors only, thread-id validation before navigation, a bounded
  seen ledger for unread, a sent ledger for idempotency, verified sends by exact
  body match, and hard stops on login challenges, captchas, account warnings,
  rate limits, and unexpected DOM. See
  [ADR 0008](docs/decisions/0008-x-browser-adapter.md).
- **Scoped autopost** for original timeline posts only: requires both
  `allow_send` and `allow_autopost`, a clear kill-switch file, UTC quiet hours,
  a daily UTC cap counting manual and autonomous posts, and a deterministic
  quality floor. Any failing rail falls back to the approval queue. See
  [ADR 0009](docs/decisions/0009-scoped-autopost.md).
- **`herald autopost status|on|off`** and **`herald x login`** (visible window,
  human sign-in, no cookie or credential access).
- **`opencode` inference backend** (`opencode run --standalone`, one-shot,
  never `--auto`) with a `doctor` version probe.
- **`create_post` operation** with timeline target, deterministic post phrases,
  policy rails, and three eval cases (45 total).
- **Target and question in the classifier** (model ladder 2/10): the grammar now
  carries `target` and `question` alongside the flag and operation, so the model
  can phrase what a request is about without ever naming an identifier. An empty
  target falls back to the deterministic hint, a question reaches the user only
  when the clarification flag is set, and resolution, validation, and approval
  are unchanged.

### Changed

- X writes are paced at least 60 seconds apart, capped at 6/hour and 20/UTC-day
  from the sent-ledger, and identical text is refused for 24 hours. A
  `profile_dir` that looks like a shared browser profile is refused before
  launch, and `x.read_only` blocks every write. These are local precautions,
  not evasion.
- Third-party debug logs are capped so `--log debug` cannot print session
  cookies or page HTML (chromiumoxide, html5ever, hyper, reqwest, ...).
- Stale Chrome profile locks are reclaimed, navigation and shutdown are
  bounded, and a crashed browser cannot hang `herald providers`.
- `herald x login` opens ordinary Chrome on the dedicated profile instead of a
  CDP session, because Google's OAuth refuses automation-controlled browsers.
  Herald's own browser no longer passes `--enable-automation`.
- Live X reads verified against a signed-in profile: mentions, status threads,
  and the home timeline parse real posts and replies with stable ids and
  timestamps. `herald inbox --scope feed` reads the home feed and `--scope notifications`
reads likes, reposts, follows, and system notices. The DM
  conversation list is still unverified and reports nothing rather than
  guessing. Home feed, richer notifications, and a GUI are on the roadmap.
- `ProviderId::X` is now implemented. `herald providers` performs a real
  browser health check for `x` when `[x]` is configured.
- Eval set grows from 42 to 45 cases; the rules engine scores 45/45.

### Fixed

- The managed `llama-server` sidecar now runs with `--jinja` and asks the chat
  template not to think. Without it the pinned reasoning model spent the whole
  token budget in its reasoning block and returned empty content, so the
  grammar never constrained the answer. Classifier calls on the pinned model go
  from tens of seconds and a parse failure to a schema-valid object in a few.
- A `llama-server` that exits early or misses the startup deadline now reports
  its last stderr lines instead of a bare "did not become ready in time".

### Not included

- Post metrics, scheduling, threads, likes, follows, and delete. Metrics in
  particular need a new operation and an ADR; the seven-method provider surface
  is unchanged.

## [1.0.0] - 2026-11-04

The first stable release. Herald reads local mail, drafts with a local model,
and sends only after a per-message human approval.

### Added

- **Maildir provider.** Reads `new/` and `cur/` from a Maildir maintained by
  another sync tool, reconstructs threads from `References`/`In-Reply-To`,
  extracts text bodies from MIME, and deduplicates on the RFC 5322 `Message-ID`
  (or a stable synthetic id when the header is absent).
- **SMTP sending for Maildir**, with `starttls`, implicit `tls`, and loopback
  `none` transports. Credentials are read from the environment; a verified copy
  of every sent message is written to the configured Sent mailbox.
- **Approval queue** with `herald approvals`, `herald approve`, `herald reject`,
  and `herald reconcile`. An approval records the provider, resolved thread,
  exact body, and SHA-256 body hash; the target is re-resolved and the hash
  re-verified immediately before sending.
- **Idempotent client tokens.** Every prepared action carries its action id, so
  a repeated `send_reply` returns the existing receipt instead of sending again.
  `reconcile` resolves an `uncertain` action through `find_sent` and never
  retries the send.
- **Configuration file** at `--config`, `$HERALD_CONFIG`, or the platform config
  directory. Precedence is command-line flag > environment variable > file >
  built-in default. `herald config get`, `list`, `path`, and `init` inspect it.
- **JSON output.** A global `--json` flag emits one object on stdout
  (`{"ok":true,"command":"...","data":...}` on success, a structured error
  object on failure) for scripting.
- **`herald init` and `herald doctor`.** `init` creates the data directory,
  database, and a starter config without writing a password or touching the
  network. `doctor` checks storage, configuration, provider health, and the
  inference backend, with `--json` output.
- **Inference backends.** Managed local inference through the pinned,
  checksum-verified `llama-server` sidecar, plus an OpenAI-compatible client for
  local servers such as Ollama, LM Studio, and vLLM. `--engine rules|auto|model`
  selects the planner route.
- **Local model management.** `herald model download|verify|status|pin` resolves
  the pinned artifact, verifies its SHA-256, and can pin an existing GGUF file
  with provenance.
- **CLI surface:** `ask`, `plan`, `inbox`, `thread`, `approvals`, `approve`,
  `reject`, `reconcile`, `history`, `audit`, `providers`, `config`, `model`,
  `eval`, and shell `completions`.
- **Project infrastructure:** CI across Linux, macOS, and Windows; MSRV check at
  1.98.0; Clippy, rustfmt, rustdoc, `cargo-deny`, and RustSec audit gates; a
  tagged release workflow with per-platform archives and `SHA256SUMS`; issue and
  pull-request templates; contribution, security, privacy, support, and code of
  conduct documents; `install.sh`; and CITATION metadata.

### Changed

- **Planner is rules-first with a model fallback.** A deterministic router
  handles common commands; the local model is consulted only when the rules
  decline and only emits a typed, validated `ActionPlan`.
- **Storage adds a `pending_approvals` table and action resolution columns**
  through an idempotent migration. Message, action, and audit data from earlier
  previews remain readable.
- **Provider attribution** is passed through storage instead of assuming the
  fake provider, so Maildir messages are recorded under `maildir`.
- **The default policy is unchanged:** `allow_send = false`, `max_body_chars =
  8000`, and a 72-hour approval TTL. Enabling sends produces an approval; it
  never enables auto-send.
- **Documentation** now covers architecture, configuration, providers,
  security, privacy, and the v1 design contract.

### Fixed

- A plan that legitimately needs clarification is surfaced to the user instead
  of being counted as a planner failure.
- Rejecting an approval or letting it expire deletes the retained body rather
  than leaving it in the database.
- A send whose outcome is unknown becomes `uncertain` and waits for
  reconciliation instead of being retried or reported as a success.
- Provider errors transition the action to `failed` and append an audit event;
  they never produce a false success.

### Security

- Message content is wrapped and labelled as untrusted data in every prompt,
  and prompt-injection regression tests cover each provider.
- The model cannot execute anything: plans are deserialized with
  `deny_unknown_fields` into a closed provider/operation enum and must pass
  `validate_plan` and the policy engine before execution.
- Sends cannot bypass approval. Approval records are required, targets are
  re-resolved, body hashes are verified, and the storage layer refuses illegal
  state transitions.
- Managed inference binds to `127.0.0.1` with a random per-run token; the
  runtime refuses a model whose digest does not match the pinned manifest.
- SMTP credentials are read from the environment, are never written by `init`,
  and are redacted from configuration output and logs.
- Maildir access is confined to the configured mailbox roots, and provider
  message ids are never joined into filesystem paths unchecked.

[Unreleased]: https://github.com/haroon0x/herald/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/haroon0x/herald/releases/tag/v1.0.0
