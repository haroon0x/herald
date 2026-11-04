# Changelog

All notable changes to Herald are documented in this file.

The format is based on [Keep a Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning 2.0.0](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
