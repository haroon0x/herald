# Modules

What each crate and module owns, the interface other code calls, and why it
earns its place. The house test for depth: **deleting the module would push
its complexity back onto its callers** — if nothing would be lost, merge it
away. Interfaces shrink until there is one obvious way to call each thing:
no flag, parameter, or option that every caller passes the same way, that the
module can work out itself, or that nobody reads.

The map of *where things live* is [../index.md](../index.md); the runtime
story is [system.md](system.md); behavior contracts are in the `//!` header of
every file listed here.

## `herald-core` — the rules, pure, I/O-free

Core depends on nothing in the workspace and never touches a filesystem, a
network, or a clock (it takes time/inputs as arguments). Its modules each own
one concept:

| Module | Owns one concept | External interface | Why it stays |
| --- | --- | --- | --- |
| `action` | The type ladder rungs 1–2: `ActionPlan` (untrusted) → `Action` (validated), body hashing, plan validation | `validate_plan(ActionPlan) -> Result<Action, PlanError>`, `body_hash`, `Operation`, `Target`, `Action`, `SendAction`, `DraftAction` | `validate_plan` is *the* gate between model output and providers. Fold it into the CLI and every caller reinvents the fail-closed rules. |
| `policy` | Authorization: can this action run, queue, or must it be refused | `authorize(&Action, &PolicyConfig) -> Decision` | Policy decisions are single-sourced and tested per case. Sends are `NeedsApproval` or `Rejected` — never `Allowed`. |
| `rules` | The deterministic router: instruction → plan without inference | `plan_offline(provider, instruction) -> Option<ActionPlan>`, `classification_parts(instruction, phrase) -> (Target, Option<String>)` | One function owns "what a classified request points at and says" — the phrase→`Description`, hint fallback, and verbatim body extraction. Callers never learn regexes. Post/tweet requests get the deterministic decline here. |
| `intent` | Tiered planner prompts | `classifier_system()`, `classifier_schema()`, `draft_system()` | The prompt+grammar pair must stay in lockstep (field order is load-bearing); the tests assert they agree. |
| `provider` | Provider-facing types: closed `ProviderId`, `MessageScope`, `MessageQuery`, `Untrusted<T>` | Types only; `ProviderId::as_str`/`is_implemented` | The closed enum is the rule "a model cannot invent a provider". `MessageQuery::recent/unread/scoped` are the one obvious shapes. |
| `state` | The action state machine | `ActionState`, `can_transition` | Terminal rules exist in exactly one place; storage enforces them too. |
| `error` | `PlanError` taxonomy | Error enum | Callers map policy/validation errors without string parsing. |
| `time` | `now_ms()` | One function | The one place wall-clock enters core — keeping core deterministic otherwise. |

## `herald-config` — configuration, parsed once

| Module | Owns | Interface | Why it stays |
| --- | --- | --- | --- |
| `model` | Shape, defaults, validation, env precedence, redaction | `Config::{default, validate, apply_env, policy, maildir_paths, x_profile_dir, get, flat}`, `ImapSettings`, `PolicySettings`, `XSettings`, `ModelSettings` | TOML knowledge lives in exactly one module; the CLI and `doctor` read derived views (`flat()` is the display shape). Unknown keys fail closed — a typo can never silently turn a policy off. |
| `paths` | Path resolution | `default_config_path`, `default_data_dir`, `ensure_private_dir`, `expand_tilde` | Platform differences (XDG vs AppData) in one place. |
| `template` | The `herald init` starter file | `TEMPLATE` | Kept parseable: a test parses it so it can never drift into an invalid file. |

Env override split (`apply_imap_env`, `apply_x_env`) keeps `apply_env`
readable; precedence is CLI > env > file > default, and secrets are never
stored by `init`.

## `herald-storage` — durable state, enforced in SQL

One module (`lib.rs`) plus `schema.sql` migrations. Owns: message dedupe on
`(provider, message_id)`, action records, the approval rows (with body
retention *until decision or expiry*), the state machine enforced in
`transition_action`, the append-only audit log, and preferences. Interface
is ~15 small methods on one `Store` type — the CLI never builds SQL.

Why it stays: the state machine is enforced at the storage layer, so a bug
in the CLI cannot mark an unapproved send `succeeded`; and expired approvals
are cancelled and their bodies deleted by the same cancel-and-audit path
before commands run. Deleting it would scatter SQL and state rules across the
CLI.

## `herald-runtime` — the model, three backends

| Module | Owns | Interface | Why it stays |
| --- | --- | --- | --- |
| `inference` | The `Inference` trait + adapters | `complete_json`, `complete_text`; `LlamaSidecar`, `OpenAiClient`, `FallbackInference` | Backends are hidden behind two methods; callers cannot tell which backend answered. The lazy-fallback wrapper owns "start the GGUF only when the primary fails". |
| `sidecar` | Managed `llama-server` lifecycle | `LlamaSidecar::start(&SidecarConfig)`, `find_on_path` | Loopback-only bind, per-run token, shutdown on drop — all in one place. |
| `opencode` | The `opencode run --standalone` backend | `OpenCodeInference`, `DEFAULT_MODEL` | Owns "never `--auto`", the banner strip, and the JSON slicing; callers pass prompts, not flags. |
| `manifest` | Pinned model checksum verification | `ModelManifest::load` | Digest/mismatch policy in one testable place. |
| `download` | Resumable artifact download | Range-resume with restart-on-ignore | Retry semantics belong with the downloader, not the CLI. |

## `herald-provider` — capabilities, per-provider modules

The trait (`CommunicationProvider`: `health`, `list_recent`, `get_thread`,
`prepare_reply`, `send_reply`, `find_sent`) is the entire surface — no
selector, URL, script, or raw request ever crosses it.

| Module | Owns | Interface | Why it stays |
| --- | --- | --- | --- |
| `fake` | Deterministic demo provider for tests/evals | `FakeProvider::with_state`, failure hooks | A scriptable in-memory world; the only place failure injection lives. |
| `maildir` | Maildir parsing, threading, SMTP send + Sent verification | `MaildirProvider::new`, `MailTransport` test seam | Provider-specific quirks (flags, threading, Sent semantics) stay in this module per the house rule. |
| `imap` | Gmail IMAP reads | `ImapProvider::new`, `ImapProviderConfig` | Session-per-operation (no daemon state), header-first reads, References threading, `\Seen` STORE on open. Sends are `NotConfigured` by construction. |
| `x` | The X browser adapter (the only browser automation) | `XProvider`, `XClient` test seam, `login`, `detect_chrome` | All selectors/URLs are constants here; page text becomes `Untrusted` and never steers navigation. |
| `lib` | The capability boundary | `PreparedAction`, `ExecutionReceipt`, `ProviderError`, `ProviderHealth` | Error taxonomy and receipt types are shared so the CLI can reason about them without provider knowledge. |

Provider-specific parsing never leaks across modules: `sender_of`-style
helpers are duplicated where providers parse differently, deliberately.

## `apps/agent` — the binary, ruling on process, owning no domain rules

| Module | Owns | Interface | Why it stays |
| --- | --- | --- | --- |
| `cli` | Argument parsing (clap) | `Cli`, `Command`, `ScopeArg` | The surface is typed; parsing errors are exit code 2, not domain errors. |
| `commands/*` | One module per command group + dispatch | `dispatch(app, Command)` | `commands/mod.rs` is the single dispatcher; policy computation happens there (`policy()`), but no domain rules — core decides, the CLI measures. |
| `registry` | Adapter construction per configured provider | `Registry::build(&Config, data_dir)` | The "configure → build-or-warn" rule (missing password, missing Chrome, uninitialised profile) lives in exactly one place. |
| `planner` | Tiered planning | `build_model`, `plan`, `plan_from_classification`, `draft_missing_body` | The only file that knows the three tiers and where each answer comes from. |
| `send` | Execution + the approval queue + reconciliation | `execute_allowed`, `queue_send`, `approve`, `reject`, `reconcile`, `record_action`, `operation_name` | The one path from a validated action to a verified receipt; idempotency and fail-closed resolution live here and nowhere else. |
| `ui` | The loopback server | `serve(cli, port, open)` | Spawns the same binary with `--json`; contains no domain logic. Endpoint handlers are one-liners over `Context::run`. |
| `output`, `error`, `app` | Human/JSON rendering, typed errors, app context | — | Presentation is separated from behavior; `--json` is a stable contract. |

## `apps/desktop` — the shell (separate workspace)

`src/main.rs` owns exactly one thing: spawn `herald ui --port 0`, parse the
token'd URL from its first stdout line, load it in a WebView, kill the child
on drop. `src-ui/` is a placeholder the Tauri build requires to exist.
No IPC, no commands, no UI of its own — it is a window, not a second Herald.
The crate is its own workspace deliberately: it links WebKitGTK, which the
main workspace's CI must not install.

## What "deep" looks like here (recent examples)

- `rules::classification_parts` replaced `target_hint` + `body_hint`: two
  shallow helpers became one obviously-called function that owns "what a
  classified request points at and says".
- `prepare_and_send` lost its `lane: &str` parameter when the autopost lane
  died; the audit-prefix special case went with it.
- `search_terms` returns `Option` instead of `""` for the removed timeline
  target, so the dead-target rule is enforced in one place, not by an
  empty-filter guess.
- Deleted outright: `used_fallback` (existed only to disarm autopost),
  `Operation::is_outbound`, `Decision::requires_approval`, `XItem::status_id`,
  `clean_draft`, the whole `quality` module, `count_actions_by_operation_since`.
  Each was either unused or existed for one removed concept.

Where a module here is accused of being shallow, the defense is: removing it
would push provider-specific or safety-critical behavior back onto callers
who must not know it (resolution, policy, idempotency, secret redaction).
