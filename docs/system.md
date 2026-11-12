# System

How Herald runs end to end: the processes, the data flows, and what happens
when each part fails. For structure and crate layout see
[architecture.md](architecture.md); for the repo map see [../index.md](../index.md);
for per-module ownership see [modules.md](modules.md).

## The one-sentence model

A person types a sentence. Herald turns it into a typed `ActionPlan`, Rust
validates and authorizes it, and the provider executes it — or stops at a
durable approval first. **The model classifies; Rust decides; the provider
executes; SQLite remembers.**

## Processes

There is exactly one Herald process model, plus three optional children:

| Process | What it is | Lifetime |
| --- | --- | --- |
| `herald` | The CLI (`apps/agent`). Parses args, plans, authorizes, executes, prints human or JSON output. | One per command. Exits after printing. |
| `herald ui` | A loopback HTTP server started by the same binary. Serves the page and, per API call, shells out to a *fresh* `herald --json` subprocess. | Until Ctrl+C. |
| Desktop shell | The Tauri app (`apps/desktop`). Spawns `herald ui --port 0`, reads the URL from the child's first stdout line, shows it in a WebView. | Until the window closes; kills the child on drop. |
| Model sidecar | A `llama-server` child (managed backend), or an `opencode run` subprocess per call (opencode backend), or a remote OpenAI-compatible request (openai backend). | Sidecar: per process that needs it. Opencode: one-shot. OpenAI: request/response. |
| Chrome | One browser (X provider only), launched lazily per operation on the dedicated profile. | Closed after each command unless `close_after = false`. |

Key consequences:

- **No daemon, no scheduler, no polling.** Nothing runs unless a command runs.
  The UI caches reads client-side for five minutes, but never refreshes them.
  `contrib/systemd/herald-ui.service` keeps the *UI process* alive for a user
  who wants it on login; it still does no background work, and every API call
  it serves is a fresh `herald` subprocess. It is a convenience wrapper, not a
  scheduler.
- **No second execution path.** The UI never touches the database or a
  provider; the desktop shell never renders its own UI. Both delegate to
  `herald --json`, so the approval queue, hash checks, and idempotency are
  the CLI's own.
- **Model output never touches a provider.** Plans are data until Rust
  validates, resolves, and authorizes them.

## Data flows

### Read (e.g. `herald inbox`)

```
ask/read command
    → plan (rules, or model classification)
    → validate_plan → authorize (Allowed for reads)
    → provider.list_recent (or get_thread)
    → record_summaries (SQLite dedupe on (provider, message_id))
    → print human / --json
```

Reads are idempotent and local: the provider returns summaries; the store
records them; repeated scans report zero new items. Bodies are fetched only
when a thread is opened, and stored bodies are not retained by default.

### Ask with the model (`herald ask "…" --engine auto|model`)

```
instruction
    → rules::plan_offline (deterministic; always wins on a hit)
    → else classifier (grammar-constrained enum: operation, clarification flag,
      target phrase, question)
    → core::rules::classification_parts (model phrase, else deterministic hint;
      quoted text becomes the body)
    → validate_plan → authorize
```

The classifier cannot emit an identifier: its `target` is a phrase, resolved
into a `ThreadId` by the provider (§ "Target resolution"). The body, when one
exists in the instruction, is taken verbatim — never rewritten.

### Send with approval (`herald ask "reply exactly: …" --enable-send`)

```
plan → validate_plan (fails closed on unknown fields, empty body, timeline)
    → authorize: allow_send? → NeedsApproval(ApprovalRequest)
    → queue_send: resolve target to a ThreadId, record action, write approval row
      (exact body + SHA-256 + expiry), state planned → awaiting_approval
    → user reviews body + hash
    → herald approve <id>:
        state check → expiry check → re-resolve target (stop if changed)
        → re-check body hash (stop on mismatch)
        → prepare_reply (read-only provider check) → send_reply (client token)
        → verify (exact body match against the user's own messages / Sent copy)
        → receipt recorded; state → succeeded | failed | uncertain
```

**`uncertain` is never retried.** A timeout after delivery may have reached
the provider; only `herald reconcile <id>` (which asks the provider via
`find_sent`) may resolve it, and it never sends.

### Draft (`herald ask "draft a reply …"`)

Validated as `Action::Draft`, prepared locally, printed with its body hash.
No provider is called. The draft text flows through the same validation as a
send when the user later asks to send it.

### Target resolution (provider-side, never the model)

| `Target` | Resolution |
| --- | --- |
| `Thread { thread_id }` | Used as-is (re-resolved at approve time; mismatch = stop). |
| `LatestFrom { sender }` | Latest summary whose sender matches (substring, case-insensitive). Single fallback: none — ambiguous or missing is an error. |
| `Description { description }` | Search terms stripped of boilerplate → provider search → distinct thread ids. Zero = `unresolved_target`, several = `ambiguous_target`. |
| `Timeline` | **Removed.** Exists only so old queued approvals decode in the queue. Every path that acts on it fails closed. |

## Storage and retention

All state lives in one SQLite file (`<data-dir>/herald.db`, WAL mode):

- `messages` — dedupe on `(provider, message_id)`. Metadata summaries only.
- `actions` — every plan with its operation, target, hashes, and state.
  States enforce the machine
  `planned → awaiting_approval → executing → succeeded | failed | uncertain`
  (plus `cancelled`) at the storage layer, not just in prose.
- `pending_approvals` — exact bodies, **deleted on decision or expiry**.
- `audit` — append-only event log; never contains bodies or secrets.
- `preferences` — small key/value store (per-provider read caching).

Minimal retention is a rule: full message bodies are never stored
indefinitely; the summary copies are metadata only.

## Trust boundaries

The full adversary model with per-threat controls and proving tests is
[threat-model.md](threat-model.md). The load-bearing ones:

1. **Model output is untrusted data.** `ActionPlan` is deserialized with
   `deny_unknown_fields`; unknown operations fail closed; the classifier
   grammar cannot emit identifiers; drafting is plain text but validated
   before use.
2. **Message content is data, not instructions.** Retrieved text enters
   prompts inside untrusted blocks and never influences selectors, URLs, or
   operations (proven by injection fixtures per provider).
3. **Identifiers are never invented.** Providers only act on validated
   thread ids; X validates `mention:<digits>` / `dm:<safe-id>` before
   navigation; resolution failures are errors, not guesses.
4. **Every send needs an approval.** `allow_send` (default false) only
   enables creating an approval; `herald approve` is the only path to the
   wire. There is no posting lane and no auto-send (ADR 0011).
5. **Secrets never surface.** Passwords come from the environment, are
   redacted in debug output and `config get/list`, and `doctor` warns on a
   plaintext `password` key.
6. **Browser automation is stop-on-surprise.** Captchas, challenges, account
   warnings, rate limits, and unexpected DOM are hard stops: `NotAuthenticated`,
   `UnexpectedLayout`, `RateLimited`, `Timeout` — never retried around.

## Failure modes

| What fails | The user sees | The state machine does |
| --- | --- | --- |
| Unknown plan field / operation | `invalid_plan` | Nothing recorded. |
| Unresolvable target | `unresolved_target` / `ambiguous_target` | Planned → failed. |
| Body missing where required | Clarification question | Nothing recorded. |
| `allow_send = false` | `policy_rejected` | Planned → failed. |
| Approval target or body changed | `approval_conflict` | Awaiting → failed. Approval body deleted. |
| Delivery timeout | `uncertain` | Executing → uncertain. Approval body deleted. `herald reconcile` is the only way out. |
| Stale `executing` (>60 s) | next command recovers it to `uncertain` | Never retried. |
| Model unavailable (auto mode) | Rules-only continues with a warning | Nothing sent. |
| Provider missing / wrong adapter | `no_adapter` or `deferred` | Nothing fabricated. |

## Extension points

- **New provider** → implement the seven `CommunicationProvider` methods in a
  new module of `herald-provider`; pass the checklist in
  [providers.md](providers.md#adding-a-provider); wire `ProviderId` in core,
  config in `herald-config`, adapter construction in `registry.rs`, and the
  `doctor` probe. A new operation on the trait itself is a design change
  (ADR + core enum + eval update), not a provider change.
- **New operation** → ADR, `Operation` variant, `validate_plan` arm, policy
  check, classifier enum + prompt, rules pattern, eval cases, and a proving
  test.
- **New policy gate** → core decides; the CLI computes measured inputs and
  passes them in; core stays pure. Every rail fails closed with a test.
- **New CLI command** → one module in `apps/agent/src/commands/`, registered
  in `commands/mod.rs`, human + `--json` output, stable exit code, docs in
  [cli.md](cli.md).

## Intentionally absent

Auto-send of any kind; a posting lane (ADR 0011); engagement automation;
a background scheduler; multi-user anything; sync; attachments handling
(listed only, in maildir); delete/archive operations; a second execution
path in the UI; `herald serve` over a network interface. Each removal is a
design decision with its own ADR, not a missing feature — and widening any
of them requires a new ADR, not a config flag.
