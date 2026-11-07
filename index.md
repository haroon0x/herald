# Herald — repo map and working standards

Read this before opening any file. It tells you where things are, which
invariants hold, and what "good" looks like here. The rules of conduct for
agents are in [AGENTS.md](AGENTS.md); the reasoning behind the architecture is
in [docs/architecture.md](docs/architecture.md). This file is the map, not the
manual.

## What this repository is

Herald is a local-first communication agent. The model emits a typed
`ActionPlan`; Rust validates, authorizes, resolves, and executes. Outbound
actions exist only after a durable, human-verifiable approval — there is no
posting lane and no auto-send. Nothing sends itself.

## Reading order for any task

1. This file.
2. [AGENTS.md](AGENTS.md) — non-negotiables and engineering rules.
3. [docs/README.md](docs/README.md) — the documentation index, by task.
4. The one crate your change belongs to (see the map), plus its tests.

## The map

### Binaries

| Path | What it is | Responsibility |
| --- | --- | --- |
| `apps/agent/` | `herald` CLI and `herald ui` | Argument parsing, the execution loop, human/JSON output, exit codes, and the loopback UI server. Owns **no domain rules**. |
| `apps/desktop/` | Tauri v2 shell (separate workspace) | Spawns `herald ui --port 0` and shows its token'd page in a WebView. Links WebKitGTK on Linux; kept out of the main workspace so CI needs no system GUI libraries. |

### Libraries

| Path | Depends on | Responsibility |
| --- | --- | --- |
| `crates/herald-core/` | nothing in the workspace | Types of the action ladder (`ActionPlan` → `Action` → `Decision`), validation, policy/authorization, state machine, prompt construction, planner rules. **No I/O, ever.** |
| `crates/herald-provider/` | core | `CommunicationProvider` capability trait, `fake` adapter, `maildir` adapter (+ SMTP), the X browser adapter. Provider-specific parsing stays inside each module. |
| `crates/herald-storage/` | core | SQLite: schema migrations, message dedupe, action records, state-machine enforcement, audit log, pending approvals. |
| `crates/herald-config/` | core | `config.toml` model, precedence (CLI > env > file > default), secrets redaction, path resolution. |
| `crates/herald-runtime/` | core | Model manifest + verification, resumable download, managed `llama-server` sidecar, OpenAI-compatible client. |

### Everything else

| Path | Purpose |
| --- | --- |
| `docs/` | Contracts, ADRs, threat model. Read before changing behaviour users rely on. |
| `eval/commands.json` | The 42-case planner eval set. Changes to operation semantics change this file. |
| `models/manifest.json` | Pinned model manifest (managed backend). |
| `experiments/wsp/` | Concluded weight-space-programming pilot. Historical; not product code. |
| `tests/` | Cross-crate fixtures. Provider contract tests live in `crates/herald-provider/tests/` and the CLI suite in `apps/agent/tests/`. |
| `.github/workflows/ci.yml` | fmt, clippy, tests, doc build. Keep it green. |

### "Where is X?"

| You want | It lives in |
| --- | --- |
| The operation enum, plan validation, body-hash | `herald-core/src/action.rs` |
| `authorize` and approval policy | `herald-core/src/policy.rs` |
| `x login` | `apps/agent/src/commands/x.rs` |
| The deterministic planner (no model) | `herald-core/src/rules.rs` |
| The state machine | `herald-core/src/state.rs` |
| The provider trait and capability contract | `herald-provider/src/lib.rs` |
| Maildir parsing, threading, Sent verification | `herald-provider/src/maildir.rs` |
| IMAP reads (sessions, headers, flags, References threading) | `herald-provider/src/imap.rs` |
| X parsing, selectors, ledgers, Chrome client | `herald-provider/src/x.rs` |
| Approvals, reconciliation, execution | `apps/agent/src/send.rs` |
| CLI command dispatch | `apps/agent/src/commands/mod.rs` |
| Local browser UI and its assets | `apps/agent/src/ui.rs`, `apps/agent/src/ui/` |
| Config file format and precedence | `herald-config/src/model.rs` |
| SQLite schema and migrations | `crates/herald-storage/schema.sql` |
| Managed model sidecar | `herald-runtime/src/sidecar.rs` |

## Working standards

These are the house rules. Code that violates them is not merged, whatever it
does.

### Boundaries

- **Dependencies point one way**: apps → crates → herald-core. Core never
  imports a workspace crate, a filesystem, a network, or a clock (use
  `time::now_ms()`). If core needs data, it arrives as a function argument.
- **The model produces plans; Rust disposes.** No code path may let model
  output skip `validate_plan` → `authorize`.
- **A provider implements the seven trait methods** and nothing else. A new
  operation on the provider is a design change: ADR, core enum change, eval
  update.
- **Provider-specific quirks stay in that provider's module.** No shared
  "general" parsing helpers across providers.
- **The CLI owns no domain rules.** Validation lives in core; policy lives in
  core; the state machine lives in storage.

### The type ladder

`ActionPlan` (untrusted) → `Action` (validated) → `Decision` (authorized) →
`Approval` (durable, hashed) → `PreparedAction` (provider-side draft) →
`ExecutionReceipt` (verified). Never skip a rung. Never carry unvalidated data
across a boundary marked with a newtype.

### Safety invariants — each has a proving test

| Invariant | Where it is enforced / proven |
| --- | --- |
| Unknown fields/enums fail plans closed | `ActionPlan` `deny_unknown_fields`; `action.rs` tests |
| The body is never rewritten | `validate_plan`; `action.rs` body test |
| Identifiers are never invented | `Target` kinds; resolution in `apps/agent/src/send.rs` |
| Sends need approval; nothing auto-sends | `policy.rs`; `ask.rs` tests |
| `uncertain` is never blind-retried | `send.rs` reconcile; provider idempotency tests |
| State machine rejects illegal transitions | `herald-storage` `transition_action` |
| Message content is data, not instructions | prompt construction; injection corpus in evals |
| Secrets never appear in output/config | `herald-config` redaction; doctor warnings |

A change that weakens any of these requires an ADR and explicit owner sign-off.

### Code

- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked
  -- -D warnings`, `cargo test --workspace --all-targets --locked` are the gate.
  `just check` runs all of them plus docs.
- `unsafe` is forbidden workspace-wide. `cargo deny check` when dependencies
  change.
- Errors: no `.unwrap()`/`expect` in library paths that can fail at runtime;
  errors map to the `ProviderError` taxonomy or are locally fatal with context.
- `tracing` for diagnostics; human output only in the CLI layer; `--json`
  envelope is a stable contract — additive changes only.
- No panics across awaits; no blocking I/O in async contexts without a worker.
- Files carry `//!` module docs stating what they own and what they never do.
- Types are closed enums or newtypes. Stringly-typed identifiers are bugs.

### AI-slop rejection list

Do not: add a capability the planner can name; retry an `uncertain` send; fall
back to a guess when resolution fails; swallow an error into `Other`; add
`#[allow(clippy::...)]` without a written reason; add a dependency without a
purpose and a license check; "temporary" special cases; dead code kept for
comfort; comments describing *what* the code does instead of *why*.

## How to add a thing

### A provider

Follow [docs/providers.md](docs/providers.md#adding-a-provider) in order. The
checklist is ordered; do not start at item 6. Reuse the
`CommunicationProvider` trait, the `MailTransport`-style test seam, and the
sent-ledger pattern. Provider ships only when the safety tests pass against it.

### An operation

1. ADR (why this operation).
2. `Operation` variant + `validate_plan` arm + policy arm + CLI `operation_name`.
3. Rules-router pattern and eval cases (`eval/commands.json`).
4. Classifier schema enum and prompt (if the model may emit it).
5. Tests proving the invariant it relies on.

### A policy gate

Core decides, CLI supplies measured inputs; core stays pure. Every rail
fails closed and has a test proving it.

### A CLI command

One module in `apps/agent/src/commands/`, registered in `commands/mod.rs`,
`--help` text, `--json` output shape, stable exit code, docs in
[docs/cli.md](docs/cli.md).

### An ADR

`docs/decisions/NNNN-slug.md`: context, decision, why, tradeoffs, consequences,
and when to revisit. Referenced from the code that implements it.

## Gates before every commit

```
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
cargo deny check        # when dependencies changed
```

Or: `just check` (+ `just deny`). CI runs the same on every push.

## Volatile state

Release status and current gaps live in [CHANGELOG.md](CHANGELOG.md) and
[docs/roadmap.md](docs/roadmap.md). This file has none, deliberately.

If a transient milestone handoff exists, it is named `HANDOFF.md` at the repo
root and is deleted when that milestone lands.
