# Roadmap

Carried over from `plan.md`, updated to the scaffolded reality.

## Phase 0: feasibility spikes (in progress)

1. [x] Typed action model, policy, and state machine (this scaffold).
2. [ ] Download Qwen3.5-2B Q4_K_M, pin digest/revision in `models/manifest.json`,
   load it via `herald-runtime`, measure cold/warm latency and memory.
3. [ ] Compare against Qwen3.5-0.8B as the low-memory fallback.
4. [~] Strict-JSON harness: 42 realistic commands in `eval/commands.json`,
   covering op selection, read vs. send, ambiguity, and prompt injection.
   Rules score 32/42: the router declines the 10 clarification cases, and a
   decline is not a question, so those count as failures rather than free
   passes. Model scoring is pending a downloaded model.
5. [ ] `chromiumoxide` spike: dedicated profile, read a harmless test page.
6. [~] Weight-space programming spike (see `experiments/wsp/`): can a
   no-gradient weight edit fix a measured classifier failure? Answer so far is
   no for this failure, and the read side found the real cause: the classifier
   asked for `operation` before `needs_clarification`, so the clarification
   decision was made at a position where the signal is absent. Results and
   protocol are in `experiments/wsp/FINDINGS.md`.

**Exit:** a real model plan parses into `herald-core` types and the Gmail spike
has a verdict.

## Phase 1: safe agent shell (scaffolded, needs the model)

- [x] Workspace, four crates, CLI, SQLite storage, audit log.
- [x] Deterministic rules planner for common commands.
- [x] Wire `herald-runtime` into the CLI behind `herald plan --model` with the
  rules router first and the planner model as fallback.
- [x] Action records created and transitioned by the real flows: allowed reads
  execute against the adapter (`planned -> executing -> succeeded | failed`),
  persist deduped message metadata, and drafts are prepared locally. Sends
  still stop at `awaiting_approval` by design.

**Exit:** a command produces a plan from either route and no browser access.

## Phase 2: first real provider, read-only

- Dedicated Chrome profile under `profiles/`, manual sign-in, CDP only.
- Gmail (or X, pending the Gmail ToS spike) page reader with stable ids and
  dedupe through `herald-storage::record_message` (the execution loop and
  dedupe path already work against the fake provider).
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

## Next task: give the model the target (2/10)

Recorded 2026-11-04. Not started. This is the first step on a deliberate ladder
away from "Rust decides everything", and it is *deliberately small*.

Ladder (self-assessed, 1 is where we are):

| level | the model controls | Rust controls |
| --- | --- | --- |
| 1/10 (now) | one of 7 labels + yes/no | arguments, resolution, execution |
| 2/10 (this task) | + the phrase: what the request is about, and how to ask | resolution, validation, execution |
| 3/10 (next) | + one lookup before deciding | how the lookup works, hard 2-call cap |
| 4-6/10 | a real multi-step loop, `ask_user` resumes a turn | tool registry, per-call policy |

**Why 2/10 before 3/10:** a lookup is only worth paying for if the model can
name what it found afterwards. Doing 3/10 first buys two model calls and no
better target.

**Change:** `classifier_schema()` grows from two fields to four —
`target` (a human phrase, never an id) and `clarification_question` (empty when
not asking). `Target::Description` already models a phrase, `validate_target`
already gates it, and an empty `target` falls back to `rules::target_hint`, so
the model can improve the target but cannot break the pipeline by omitting it.
`main.rs` needs no change: `validate_plan` already prefers
`plan.clarification_question` over its hardcoded default.

**Steps**

1. Schema + prompt in `crates/herald-core/src/intent.rs`.
2. Parse and fall back in `apps/agent/src/engine.rs::plan_via_classifier`.
3. Tests: four fields present, prompt asks for them, empty target falls back,
   a model-authored question reaches `PlanError::ClarificationRequired`.
4. `target_contains` in `eval/commands.json` plus scoring, so target quality is
   measured rather than assumed. Today the eval scores operations only.
5. Measure: classifier score unchanged (32/42), 20 targets human-read, and count
   the cases where the model's target beats the regex.
6. Go/no-go for 3/10.

**Gate before 3/10** — all four:

- no regression on the 42-case classifier score;
- `target_contains` reported by the harness;
- >=70% of 20 sampled targets are specific enough for the draft prompt to use;
- >=3 cases where the model beats the regex (requests with no " from ").

If the last two fail on the 0.8B, upgrade the model *before* 3/10 rather than
tuning the prompt.

**Out of scope:** lookup steps, tool registry, loops, memory, and any change to
`validate_plan`, `authorize`, the state machine, or the rules router.

## CDN of decisions

- *First provider:* decided by the Phase 0 Gmail spike. Design keeps
  `CommunicationProvider` implementation-compatible with an IMAP/OAuth adapter
  so the choice can flip without touching core.
- *No Ollama:* `llama-server` sidecar is the pinned runtime; `mistral.rs`
  stays a documented alternative evaluated only after MVP.
