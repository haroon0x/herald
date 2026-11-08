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
scheduling, a GUI, and a multi-step agent loop. The X browser adapter is the
only exception added since (see the in-progress section below).

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
| opencode free pool (glm, `--engine model`), 2026-11-07 | 35/42 (83%); fails lean `prepare_reply` where `send_reply` was expected |
| Test suite | 231 tests now; 161 in the v1.0 tree; 62 at the 2026-11-03 snapshot |

Two scorer bugs were fixed before those numbers were trusted: a decline was
being counted as a clarification, and a model error was being scored as a
success. After the fix the rules router exposed its real gap: it could decline
but could not ask. It now emits a deterministic clarification for destructive,
non-mail, missing-body, and embedded-instruction requests, so it covers the
42-case set without pretending to execute them. Declining is still not counted
as clarifying, and the model is still measured separately.

The 42-case set is unchanged in v1.0. It covers intent and refusal, not Maildir
parsing or approval correctness; those have their own tests.

## In progress, unreleased: X adapter

This work sits on top of v1.0 and is not a numbered release yet. It ships when
the live canary passes (approval-gated reply, DM to self, metrics absent by
design) and the owner has signed off.

- **X provider** (`x`): mentions and DMs over a dedicated Chrome profile,
  approval-gated replies and DMs, with constant selectors, a local seen
  ledger, a sent ledger, verified sends, and hard stops on challenges. See
  [ADR 0008](decisions/0008-x-browser-adapter.md).
- **No posting lane.** Tweet generation and posting stay in Zyquor. See
  [ADR 0011](decisions/0011-no-posting-lane.md).
- **Eval set stays at 42 cases**; the historical 45-case measurement below
  describes the removed posting work, not the current capability set.
- **`opencode` inference backend**: one-shot `opencode run --standalone
  --model <id>` subprocess, no `--auto`, no server reverse-engineering.
- **Not included:** post metrics (Zyquor's pipeline owns posting performance), scheduling,
  threads, likes, follows, or delete.

## Next after the X canary: feed, notifications, DM list, GUI

These are requested and tracked, not shipped:

- **Home feed reads.** Shipped as `herald inbox --provider x --scope feed`
  (read-only, home timeline as its own scope).
- **Notifications beyond mentions.** Shipped as
  `herald inbox --provider x --scope notifications` (likes, reposts, follows,
  system notices; display-only when there is no underlying status).
- **DMs: not planned.** The owner does not need DM reads. X renders them in an
  embedded `xchat` app outside the page DOM; Herald keeps the API scope but
  does not expose it in the UI and will not guess at the embed.
- **A GUI.** Shipped as `herald ui`: a loopback browser UI over the same
  commands and approval queue ([ui.md](ui.md), ADR 0010). The Tauri desktop
  shell (`apps/desktop/`, own workspace; spawns `herald ui` and shows its
  token'd page in a WebView) builds and bundles on Linux as a `.deb` plus an
  AppImage with the `herald` sidecar inside — see its README for the exact
  build steps and system packages (`libwebkit2gtk-4.1-dev librsvg2-dev
  patchelf`). PNG, ICO, and ICNS bundle icons are included.
- **Post metrics.** Not implemented in Herald; the Zyquor pipeline owns posting performance.

## v1.1: IMAP, read-only (implemented, not yet live-verified)

The first post-v1 release adds a third real provider without adding send
authority. Implemented 2026-11-07: Gmail IMAP via app passwords, one
short-lived session per command on a blocking worker, header-first reads,
References-rooted threads, local search, and a config-gated `\Seen` flag
write when a thread is opened. Sends stay `NotConfigured`. See
[ADR 0012](decisions/0012-imap-provider.md).

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
closed, and the injection corpus passes against headers and bodies. The
automated suite covers error mapping and config refusal without a network;
the real round trip (`herald inbox --provider imap` against a live Gmail
app-password account) is the owner's canary.

## v1.2: summaries and search

Shipped so far (2026-11-08):

- Thread summaries cite message ids. The planner's summary prompt asks for
  `[id]` citations using only the IDs shown in the thread, and `ask` reports
  `summary_citations` (the thread's actual IDs) in JSON and under the Summary
  heading in text. Citations are derived from the thread, never parsed from
  model text, so a citation-looking string the model writes stays inert.
- Search understands "from Sam", "about the lease", and "last week" without
  sending the mailbox to a model. Both sender/topic orderings already route
  through `LatestFrom` target resolution; trailing time phrases
  (`last week`, `yesterday`, and five more) are stripped so they never become
  the substring needle. No new query syntax; provider search semantics
  (sender/subject/body substring) are unchanged.

Still open:

- Local full-text search over stored metadata plus on-demand bodies, with a
  retention policy the user can see.
- Draft quality work: tone hints, reply length, and a diff view of the proposed
  body.

Search is where retention becomes a real design question. v1.2 will need an
explicit, configurable body-retention window, and it will have to be deletable.

## v1.3: packaging

Shipped so far (2026-11-08):

- Homebrew formula (`contrib/homebrew/herald.rb`), Scoop manifest
  (`contrib/scoop/herald.json`), and a Debian `.deb` builder
  (`contrib/debian/build-deb.sh`). The release workflow builds the `.deb`,
  covers it in `SHA256SUMS`, and signs `SHA256SUMS` with minisign when the
  `MINISIGN_SECRET_KEY` repo secret is set (`SHA256SUMS.sig` published next
  to the archives). See `contrib/README.md` for the maintainer steps
  (fill in per-archive hashes, publish the tap/bucket, generate the minisign
  key) and for the Debian usage.
- `install.sh` verifies `SHA256SUMS.sig` with minisign when
  `HERALD_MINISIGN_PUB` points at the project's public key; otherwise it
  falls back to HTTPS checksums, as documented in its header.

Still open: the tap/bucket publication itself (needs an org-owned repo), the
minisign key (needs the maintainer), the static/musl binary for other Linux
distributions, and a first-run demo command. `herald init` prints the demo
commands and `herald doctor` runs the full diagnosis; the demo is the fake
provider via `herald ask`.

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

Recorded 2026-11-04 and measured 2026-11-07. Schema, prompt, parsing, and tests are in place; the measurement is recorded below, and the go/no-go decision remains step 6.

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
   field for five sampled cases, and a wider human review is recorded in
   [target-review-2026-11-07.md](target-review-2026-11-07.md). The
   harness now builds its plan through `planner::plan_from_classification`, so
   the phrase it scores is the model's, not the regex's.
5. [x] Measure: **34/42** against the 32/42 baseline; **11/20** reviewed rules
   targets were marked specific; and the classifier target **beats the regex
   in 13** cases, ties in 6, and is worse in 1.
6. [] Go/no-go for 3/10.

Measured 2026-11-07 with the user's configured opencode backend: **34/42**
against the **32/42** baseline. In the fixed 20-case review, **11 of 20**
rules targets were marked specific and 9 were vague; the model target beats
the rules hint in **13** cases, ties in **6**, and is worse in **1**. See
[target-review-2026-11-07.md](target-review-2026-11-07.md). Step 6 remains
the owner's call.

The earlier 0.8B measurement remains part of the historical context: the
sidecar needed `--jinja` and thinking-off plumbing so grammar-constrained
outputs could arrive, and the pinned model regressed against the v1.0
baseline at the time. Those details are retained in the target-review record
and the earlier measurement notes; this step measures no planner or core
behavior change.

Gate before 3/10, all four:

- No regression on the 42-case classifier score.
- [x] `target_contains` is reported by the harness for the sampled cases.
- At least 70% of 20 sampled targets are specific enough for the draft prompt.
  (11/20 recorded; the threshold judgment is the owner's call.)
- At least 3 cases where the model beats the regex, meaning requests with no
  " from " phrase. (13 recorded.)

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
