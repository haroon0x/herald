# Roadmap

Carried over from `plan.md`, updated to the scaffolded reality.

## Phase 0: feasibility spikes (in progress)

1. [x] Typed action model, policy, and state machine (this scaffold).
2. [ ] Download Qwen3.5-2B Q4_K_M, pin digest/revision in `models/manifest.json`,
   load it via `herald-runtime`, measure cold/warm latency and memory.
3. [ ] Compare against Qwen3.5-0.8B as the low-memory fallback.
4. [ ] Strict-JSON harness: 30-50 realistic commands, hand-scored against the
   closed operation set (op selection, recipient resolution, read vs. send
   distinction, prompt-injection refusals, structured-output validity).
5. [ ] `chromiumoxide` spike: dedicated profile, read a harmless test page.

**Exit:** a real model plan parses into `herald-core` types and the Gmail spike
has a verdict.

## Phase 1: safe agent shell (scaffolded, needs the model)

- [x] Workspace, four crates, CLI, SQLite storage, audit log.
- [x] Deterministic rules planner for common commands.
- [ ] Wire `herald-runtime` into the CLI behind `herald plan --model` with the
  rules router first and the planner model as fallback.
- [ ] Action records created and transitioned by the real flows (the record in
  `herald plan` is currently a demonstration, not an execution loop).

**Exit:** a command produces a plan from either route and no browser access.

## Phase 2: first real provider, read-only

- Dedicated Chrome profile under `profiles/`, manual sign-in, CDP only.
- Gmail (or X, pending the Gmail ToS spike) page reader with stable ids and
  dedupe through `herald-storage::record_message`.
- Summaries via the warm model; important-message triage.
- **Exit:** repeated scans report no false "new" items, failures stop safely,
  and a provider health failure surfaces instead of crashing.

## Phase 3: draft and send

- Reply drafting, recipient re-resolution immediately before send.
- Confirmation card: provider, recipient, thread, exact body, body hash.
- Send + verification + idempotent reconciliation for timeouts.
- **Exit:** never sends to an ambiguous target, never sends without approval,
  never duplicates after a recoverable failure.

## Phase 4: resilience and release

- Model download UX (size, pause/resume), installer, quiet hours, diagnostics.
- Security review of profile handling, retention, and approvals.

## CDN of decisions

- *First provider:* decided by the Phase 0 Gmail spike. Design keeps
  `CommunicationProvider` implementation-compatible with an IMAP/OAuth adapter
  so the choice can flip without touching core.
- *No Ollama:* `llama-server` sidecar is the pinned runtime; `mistral.rs`
  stays a documented alternative evaluated only after MVP.
