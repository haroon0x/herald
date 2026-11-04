# Roadmap

This page describes what v1.0 contains, what comes next, and the order the
model's authority is allowed to grow. It replaces the Phase 0 to Phase 4 plan
from the scaffold; that history is at the bottom.

## v1.0, shipped

- Two providers at release: `fake` (seeded demo inbox) and `maildir` (local
  Maildir plus SMTP with a verified Sent copy). `gmail` and `x` were planned
  enums with no adapter; the X adapter landed after this release (see below).
- The full action ladder: `ActionPlan` to `Action` to `Decision` to `Approval`
  to `PreparedAction` to `ExecutionReceipt`.
- Approval queue with retained body, body hash, resolved thread, and a 72-hour
  expiry. `approve` re-resolves the target and re-checks the hash before
  sending.
- Idempotent writes: client token, `X-Herald-Token`, provider-side dedupe,
  Sent-folder verification, and `reconcile` for `uncertain` sends.
- The state machine with `cancelled` and `uncertain`, enforced in storage.
- TOML configuration with CLI > env > file > default precedence, environment
  secrets, and a `doctor` that checks paths, Maildir, SMTP, model, and secrets.
- Rules router plus a grammar-constrained classifier, plain-text drafting, and
  thread summaries, behind a managed `llama-server` sidecar or an
  OpenAI-compatible endpoint.
- Stable `--json` output for every command, documented exit codes, and shell
  completions.
- The 42-case eval set, including injection and ambiguity cases.
- Documentation: architecture, providers, configuration, CLI, threat model,
  FAQ, ADRs.

What v1.0 deliberately did not include: IMAP, Gmail, X, browser automation,
auto-send, attachments, archive or delete, multi-account, sync, notifications,
scheduling, a GUI, and a multi-step agent loop. X and its scoped autopost lane
are the only exceptions added since (see the in-progress section below).

## Measured, not assumed

These numbers come from `docs/status-2026-11-03.md` and the eval harness. They
measure operation selection on the 42-case set, not mail quality.

| What | Result |
| --- | --- |
| Rules engine, 42 cases | 42/42 (asks a deterministic clarification for the 10 ambiguous cases) |
| Qwen3.5-0.8B, original schema | 21/42 strict |
| Qwen3.5-0.8B, shipped flag-first prompt | 32/42 |
| Clarification recall, original schema | 0/10 |
| Clarification recall, flag-first schema | 10/10 loose rule, 4/10 with the specified rule and one false positive |
| Test suite | 161 tests in the v1.0 tree; 62 at the 2026-11-03 snapshot |

Two scorer bugs were fixed before those numbers were trusted: a decline was
being counted as a clarification, and a model error was being scored as a
success. After the fix the rules router exposed its real gap: it could decline
but could not ask. It now emits a deterministic clarification for destructive,
non-mail, missing-body, and embedded-instruction requests, so it covers the
42-case set without pretending to execute them. Declining is still not counted
as clarifying, and the model is still measured separately.

The 42-case set is unchanged in v1.0. It covers intent and refusal, not Maildir
parsing or approval correctness; those have their own tests.

## In progress, unreleased: X adapter and scoped autopost

This work sits on top of v1.0 and is not a numbered release yet. It ships when
the live canary passes (post, DM to self, metrics absent by design) and the
owner has signed off.

- **X provider** (`x`): mentions and DMs over a dedicated Chrome profile,
  replies, DMs, and original posts, with constant selectors, a local seen
  ledger, a sent ledger, verified sends, and hard stops on challenges. See
  [ADR 0008](decisions/0008-x-browser-adapter.md).
- **Scoped autopost** for original timeline posts only, with every rail
  (double opt-in, kill switch, quiet hours, UTC daily cap, deterministic quality
  floor). Replies and DMs still require approval. See
  [ADR 0009](decisions/0009-scoped-autopost.md).
- **Eval set grows to 45 cases** with three post cases; the rules engine scores
  45/45. The historical 42-case numbers above describe the v1.0 baseline.
- **`opencode` inference backend**: one-shot `opencode run --standalone
  --model <id>` subprocess, no `--auto`, no server reverse-engineering.
- **Not included:** post metrics (needs a new capability and ADR), scheduling,
  threads, likes, follows, or delete.

## Next after the X canary: feed, notifications, DM list, GUI

These are requested and tracked, not shipped:

- **Home feed reads.** Shipped as `herald inbox --provider x --scope feed`
  (read-only, home timeline as its own scope).
- **Notifications beyond mentions.** Shipped as
  `herald inbox --provider x --scope notifications` (likes, reposts, follows,
  system notices; display-only when there is no underlying status).
- **DM list selectors.** X now renders DMs in an embedded `xchat` app whose
  conversation DOM is not in the page snapshot. Mentions, status threads, and
  the home feed are verified against the live DOM; DMs report nothing until
  the embed is understood, rather than guessing.
- **A GUI.** The CLI is the only surface today. A desktop or TUI front end would
  wrap the same commands and approval queue; it must not add a second execution
  path, and every send still needs an explicit approval. The provider surface
  does not change.
- **Post metrics.** Still needs a new operation and an ADR (ADR 0008).

## v1.1: IMAP, read-only

The first post-v1 release adds a third real provider without adding send
authority.

- IMAP read-only against Gmail and Outlook using app passwords, not OAuth.
- Connection state machine: connect, authenticate, select, fetch, idle-free.
- Header-only fetch for listings, full fetch for one thread.
- Server-side search where it is reliable, local filtering otherwise.
- The same dedupe key: `(provider, message_id)`.
- No send. `send_reply` returns `NotConfigured` until SMTP submission is
  configured and tested.

Why app passwords first: a local desktop app cannot keep an OAuth client secret,
and the OAuth consent flow is a hosted-service problem. App passwords are
worse for revocation and better for a first release. OAuth is a later ADR.

Exit criteria: repeated scans report zero new items, a dropped connection fails
closed, and the injection corpus passes against headers and bodies.

## v1.2: summaries and search

- Thread summaries that cite message ids instead of paraphrasing without
  attribution.
- Local full-text search over stored metadata plus on-demand bodies, with a
  retention policy the user can see.
- Search that understands "last week", "from Sam", and "about the lease"
  without sending the mailbox to a model.
- Draft quality work: tone hints, reply length, and a diff view of the proposed
  body.

Search is where retention becomes a real design question. v1.2 will need an
explicit, configurable body-retention window, and it will have to be deletable.

## v1.3: packaging

- Homebrew tap for macOS and Linux.
- Scoop bucket for Windows.
- Debian package and an AppImage or static binary for other Linux
  distributions.
- Signed release artifacts and checksums published next to the binaries.
- A first-run flow that does not require reading the docs: `init`, `doctor`,
  and a one-command demo.

The binary stays a single executable. Packaging must not introduce a service,
a daemon, or an installer that writes outside the user's directories.

## v2.0: a capped tool loop

This is the first release where the model can ask for a second step. It is
gated on the ladder below.

- A hard cap of two provider calls per instruction, enforced in Rust.
- A tool registry with typed, named tools: `list_recent`, `get_thread`,
  `search`, `draft`. No shell, no HTTP, no browser.
- `ask_user` as a tool that suspends the turn instead of guessing.
- Per-call policy: a lookup is a read, and a send still needs an approval.
- Every loop iteration is audited with its tool, arguments, and result.

The cap is the design. An unbounded loop is how a local agent turns one
injection into a sequence of actions.

## TUI and desktop

After the loop is stable:

- A terminal UI with a thread list, a body pane, and an approval card that
  shows the hash.
- A desktop shell built on the same JSON output, not a second implementation
  of the policy.
- Notifications only for pending approvals, never for new mail.

## Encrypted sync

The last item on this list, and the one with the most open questions:

- End-to-end encrypted sync of preferences, action history, and pending
  approvals between a user's machines.
- No message bodies in the sync payload.
- Key management that does not require a Herald account.

This is where the AGPL boundary and a possible paid tier meet. The safety
defaults stay identical in every tier; a paid tier cannot buy auto-send.

## The model ladder

The long-term question is how much of the decision the model is allowed to
make. The answer is a ladder, and each rung needs a gate.

| Level | The model controls | Rust controls | Status |
| --- | --- | --- | --- |
| 1/10 | One of seven operation labels plus a yes/no clarification flag | Arguments, target resolution, validation, execution | Shipped in v1.0 |
| 2/10 | The target phrase: what the request is about, and how to ask | Resolution, validation, execution | Next model task |
| 3/10 | One lookup before deciding | How the lookup works, hard two-call cap | After 2/10 |
| 4/10 | A two-step loop over typed tools | Tool registry, per-call policy, cap | v2.0 |
| 5/10 | Choosing which read tool fits the question | Tool set, argument schema, cap | v2.0 |
| 6/10 | `ask_user` to resume a suspended turn | When suspension is legal, timeout, audit | v2.0 |
| 7/10 | Drafting with a chosen tone and length | Policy, approval, provider calls | Not committed |
| 8/10 | Proposing a send target from thread content | Re-resolution, approval, hash check | Not committed |
| 9/10 | Multi-step plans across threads | Cap, budget, per-step approval | Not committed |
| 10/10 | Nothing further | Everything | Not a goal |

Level 10 is deliberately unreachable. The last rung is the product: the model
proposes, Rust disposes, the user approves.

### Next: 2/10

Recorded 2026-11-04. Schema, prompt, parsing, and tests are in place; the
measurement is not started. The change is small and measurable.

`classifier_schema()` grew from two fields to four: `target` (a human phrase,
never an id) and `question` (empty when not asking).
`Target::Description` already models a phrase, `validate_target` already gates
it, and an empty target falls back to `rules::target_hint`, so the model can
improve the target but cannot break the pipeline by omitting it. The same holds
for an empty question: `validate_plan` supplies its default.

The JSON field is `question`, not `clarification_question`, because object keys
serialise sorted and llama-server's grammar follows that order. A name starting
with `c` would be generated *before* `needs_clarification`, forcing the model to
commit to a question string before it decides whether to ask one. `question`
sorts after `operation`, so the measured flag-then-operation sequence is
untouched and the two phrases are appended after it.

Steps:

1. [x] Schema and prompt in `crates/herald-core/src/intent.rs`.
2. [x] Parse and fall back in `apps/agent/src/planner.rs::plan`.
3. [x] Tests: four fields present, the prompt asks for them, an empty target falls
   back, a model-authored question reaches `PlanError::ClarificationRequired`,
   and a question on a false flag is discarded.
4. [x] `target_contains` cases in `eval/commands.json`. The harness reports the
   field for five sampled cases; a wider human review is still pending. The
   harness now builds its plan through `planner::plan_from_classification`, so
   the phrase it scores is the model's, not the regex's.
5. Measure: no regression against the 42-case baseline, 20 targets read by a human,
   and a count of cases where the model's target beats the regex.
6. Go/no-go for 3/10.

Gate before 3/10, all four:

- No regression on the 42-case classifier score.
- [x] `target_contains` is reported by the harness for the sampled cases.
- At least 70% of 20 sampled targets are specific enough for the draft prompt.
- At least 3 cases where the model beats the regex, meaning requests with no
  " from " phrase.

If the last two fail on the 0.8B, upgrade the model before 3/10 rather than
tuning the prompt.

Out of scope for 2/10: lookup steps, a tool registry, loops, memory, and any
change to `validate_plan`, `authorize`, the state machine, or the rules router.

## History

The project started as a scaffold with four crates, a typed action model, a
policy engine, SQLite storage, and a deterministic rules planner. Phase 0 ran
feasibility spikes: a model eval harness, a Gmail/browser spike that was never
started, and a weight-space programming pilot.

The pilot concluded that the classifier's clarification failure was a prompt
field-order problem, not a weight-level obstacle, and shipped no edit. The
details are in `experiments/wsp/FINDINGS.md`. The browser direction was dropped
in favor of Maildir and SMTP, recorded in
[decisions/0004-email-provider-maildir-smtp.md](decisions/0004-email-provider-maildir-smtp.md).
The old Phase 0 checkboxes are superseded by this page.

## Deliberately absent

Auto-send rules, a general browser agent, a shell tool, a generic HTTP tool,
attachment handling, archive or delete, multi-user, a hosted service, and any
feature that lets the model grant itself authority. Each one would need a new
ADR, a new threat-model entry, and tests that prove the safety property it
relies on.

## Related

- [architecture.md](architecture.md): how the current system is built.
- [providers.md](providers.md): the checklist a new provider must satisfy.
- [threat-model.md](threat-model.md): the adversary model the ladder has to respect.
- [decisions/](decisions/): the decisions behind each step.
---

[Herald README](../README.md) · [Security](../SECURITY.md) · [Contributing](../CONTRIBUTING.md) · [Privacy](../PRIVACY.md) · [Changelog](../CHANGELOG.md)
