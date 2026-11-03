# Architecture

## Principle

Herald is a local agent, not a dashboard and not a browser macro. One command
surface, one planner, a closed set of typed actions, per-provider adapters, and
SQLite for durable state. Everything below is enforced by code, with tests.

## Data flow

```text
User command
   |
   v
Planner (rules now, local LLM next)
   |  emits ActionPlan JSON (deny_unknown_fields, closed enums)
   v
validate_plan -> Action (Read | Draft | Send)
   |
   v
authorize(action, PolicyConfig) -> Allowed | NeedsApproval | Rejected
   |
   v
Target resolution against provider data   <---- clarification if ambiguous
   |
   v
Provider adapter (capability trait only)
   |
   v
Result verification -> ExecutionReceipt
   |
   +--> SQLite (messages, actions, audit_events, preferences)
   +--> user response
```

## Crate boundaries

- **herald-core** — domain only. Typed plans, validation, policy decisions,
  action state machine, prompt construction with the trust boundary. No I/O.
- **herald-provider** — the only place that knows what a provider is. One
  trait, seven methods (health, list, thread, prepare, send), nothing generic.
  The fake provider implements the same contract and is used by every test that
  does not need a real service.
- **herald-storage** — SQLite. Stable ids and dedupe for messages, the action
  state machine guarded at the storage layer, an append-only audit log, and
  preferences. It refuses illegal transitions; callers cannot corrupt state.
- **herald-runtime** — model manifest and checksums, loopback-only
  `llama-server` management, JSON chat completion. No knowledge of actions.
- **apps/agent** — CLI. Translates instructions into plans, applies policy,
  records actions, and runs the local execution loop for allowed reads and
  drafts. Cannot mutate an account yet.

## Trust boundaries

1. **User instruction vs. provider content.** `prompt::build_planner_prompt`
   puts the instruction in `<user-instruction>` and every retrieved message in
   `<untrusted-message id=... sender=...>` blocks, with system rules that treat
   message text as data only.
2. **The model vs. execution.** A parsed plan is inert until `validate_plan`
   accepts it. Unknown fields fail deserialization. Unknown providers and
   operations cannot even be constructed.
3. **Approval vs. execution.** `authorize` may only produce a send decision
   through `ApprovalRequest`. The MVP CLI stops at the approval record; the
   browser step that later consumes it looks up the same record by id and
   re-reads the thread immediately before sending.
4. **Optimistic retries vs. reality.** A `Timeout` after a send goes to
   `Uncertain`; only a re-read of the thread moves it forward. The idempotency
   content hash is the SHA-256 of the exact body.

## Execution loop (Phase 1)

```text
Allowed action
   |
   v
planned -> executing
   |            |
   |            +-- read: resolve target -> adapter -> record messages (deduped)
   |            |        -> succeeded | failed
   |            +-- draft: prepare locally (no provider call) -> succeeded
   |
   +-- no adapter (gmail/x): audit `deferred`, action stays planned
```

- Target resolution is deterministic and fails closed: a `LatestFrom` target
  only matches an existing sender, a `Description` becomes a trimmed search,
  and a `Thread` id is verified by the adapter. No LLM is involved here.
- Message persistence dedupes on `(provider, message_id)`, so repeated scans
  report zero new items.
- A provider error transitions the record to `failed` and appends an audit
  event; it never reports success and never retries blindly.
- `execute` refuses `Action::Send` outright. Sends only move to
  `awaiting_approval` and wait for the Phase 3 approval consumer.

## Inference runtime

- Pinned GGUF download, checksummed against `models/manifest.json`.
- One warm `llama-server` child process per app run, loopback host hardcoded,
  random per-run token, health-checked startup with a deadline and clean
  shutdown on drop.
- The manifest that ships in `models/` has placeholder digest/revision values
  until the Phase 0 spike pins a real artifact. The runtime refuses a model
  whose digest does not match.

## What is intentionally absent

- No notifications, no scheduling, no speech. These are Phase 4+.
- No auto-send rules. The model cannot grant itself authority.
- No archive, delete, post, follow, or bulk operations. Adding one is a core
  enum change plus tests, which is the point.
- No full-message storage by default. Retention stays minimal until a
  conversation view needs more, at which point retention becomes configurable
  and deletable.
