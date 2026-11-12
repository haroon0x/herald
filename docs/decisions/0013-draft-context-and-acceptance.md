# 0013: Draft from the thread, and measure whether drafts get sent

Date: 2026-11-12
Status: Accepted

## Context

Before this decision, `Model::draft` was called with two strings: a target
description ("latest message from the founder") and the user's instruction.
The email being answered was never in the prompt. The model wrote into a
conversation it could not see, so its output was structurally generic, and the
only quality numbers in the repository measured operation selection — which is
not what a person notices when they decide whether to send a draft.

The approval gate makes this a product problem, not a polish problem. A send
costs the user a review. If the draft has to be rewritten, approval overhead
makes Herald slower than typing the reply, and the safety architecture is pure
cost. The break-even is: draft quality × acceptance rate > review cost. No
number for that existed.

The context machinery already existed and was already safe: `summarize_prompt`
wraps every message in `<untrusted-thread>` with a message id, a sender, and a
character budget, and `crates/herald-core/src/intent.rs` states that retrieved
text is data. Drafting simply was not using it.

## Decision

1. **Draft after resolution.** The model receives the resolved thread's last
   three messages (newest kept first, 4,000 characters total), wrapped in
   `<untrusted-thread subject="…">` with per-message
   `<message id="…" sender="…">` blocks, and the user's instruction last.
   `herald_core::intent::draft_prompt` builds it; `Model::draft` calls it.
   The new `herald reply <thread-id>` command makes that path deterministic by
   taking the thread id from the user rather than from the classifier.
2. **The wrapper is enforced, not assumed.** Every untrusted value interpolated
   into the prompt is tag-neutralised: a body containing `</untrusted-thread>`
   or `<convey>` renders as inert text, and sender/subject attributes have
   quotes, angle brackets, and newlines escaped. Tests:
   `draft_prompt_neutralises_closing_tags_in_a_body`,
   `draft_prompt_escapes_the_sender_attribute`.
3. **A send request may be drafted, but only with `--enable-send`.** A
   `send_reply` plan with no quoted body still asks what to say when sending is
   disabled. When it is enabled, the model drafts and the body goes to the
   approval queue — the model still never sends, and the exact body is still
   shown and hashed.
4. **Provenance is recorded as hashes, not text.** When a model-drafted body is
   queued, the action row keeps `draft_hash` plus the added/removed word counts
   between the draft and the queued body. `herald doctor` reports the unedited
   acceptance rate (below 40% after five decisions is a warning). The draft
   itself is never stored: the metric must not become a second body store.

## Why

- The thread is the input the task requires. A reply without it is a
  completion; a reply with it is an answer.
- The safe path already existed. Reusing `summarize_prompt`'s wrapper and
  budget keeps one injection contract instead of two.
- Acceptance rate is measurable without human labelling and without storing
  mail: compare hashes at decision time. It is the number the roadmap should
  be gated on, replacing operation-selection scores that cannot detect a
  useless draft.

## Tradeoffs

- One extra provider read per draft. `thread_for_draft` is best-effort: a
  failure falls back to the historical context-free prompt rather than failing
  the command. For the X adapter that read is a browser navigation, which is
  the slowest part of a draft.
- Prompt size grows by up to 4,000 characters, which costs latency on CPU
  inference and tokens on a hosted backend.
- Existing 42-case eval numbers are unaffected because that harness scores
  plans, not bodies. Draft-quality numbers do not exist yet; that is the point.
- `draft_prompt` duplicates the summary wrapper pattern rather than sharing a
  builder. The two prompts differ (instruction position, message count), and a
  premature abstraction would hide the injection surface.

## Consequences

- `herald reply <thread-id> [--convey TEXT] [--enable-send]` is the
  deterministic draft → approve → send loop.
- `herald doctor` gains a `draft acceptance` line and `--json` gains
  `draft_acceptance`; both are additive.
- The historical context-free prompt is still produced when no thread is
  available, so earlier measurements remain comparable.

## When to revisit

- If acceptance stays below 20% after a month of real use, the model path is
  wrong: change the model (a larger local GGUF) or drop send drafting and keep
  triage + summaries.
- If `DRAFT_CONTEXT_CHARS` turns out to be the latency bottleneck on CPU, make
  it configurable under `[draft]` rather than raising it silently.
