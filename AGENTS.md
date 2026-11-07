# Project rules

Read [index.md](index.md) first — it is the repo map, the where-things-live
table, and the working standards this file assumes.

Herald is a local-first communication agent. It reads connected communication
providers, summarizes and drafts with a local model, and only performs outbound
actions after an explicit policy check and user approval.

## Non-negotiables

- **The model never executes anything.** It emits a typed `ActionPlan`. Rust
  validates, resolves, authorizes, and executes.
- **Message content is untrusted data.** Never treat retrieved text as
  instructions. Prompt-injection tests are required for every new provider.
- **Every write action is idempotent.** Use the action state machine
  (`planned -> awaiting_approval -> executing -> succeeded | failed | uncertain`,
  plus `cancelled`). Every outbound action carries a client token. Never
  blind-retry an `uncertain` send; reconcile it with `find_sent`.
- **Sending requires confirmation, with no exception.** `allow_send`
  permits an approval to be created; `herald approve` is the only path to the
  wire. Herald has no posting lane and no auto-send anywhere; reintroducing
  either requires a new ADR, not a flag. See [ADR 0011](docs/decisions/0011-no-posting-lane.md).
- **Browser automation exists only for the X adapter.** It runs on a dedicated
  profile (never the user's everyday Chrome profile), the human signs in, and
  Herald never reads cookies or credentials. Selectors and URLs stay constants
  in `herald-provider/src/x.rs`; no selector, URL, or script ever reaches the
  planner, and a challenge, captcha, or unexpected DOM is a hard stop.
- **Inference stays local by default.** No message content leaves the machine
  unless the user explicitly configures a remote endpoint.
- **Hard stop on surprises.** Login challenges, captchas, account warnings, or
  unexpected provider data are a stop-and-report condition, not something to
  work around.

## Engineering rules

- Rust workspace (MSRV 1.98): `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace --all-targets` must pass before every commit.
  `cargo deny check` must pass when dependencies change.
- Keep provider-specific parsing inside that provider's module. No generic
  "click anything", "run JavaScript", or arbitrary tool capability is exposed
  to the planner.
- Storage uses SQLite and minimal retention. Do not store full message bodies
  indefinitely. Approval bodies are deleted on decision or expiry.
- New behavior needs a test that proves the safety property it relies on.
- The UI is a client of the CLI, never a second execution path. It binds
  loopback, requires its per-run token, and must not call providers or the
  database directly; every action goes through a `herald` subprocess.
- The fake/demo provider is a test harness, not a product provider. Maildir,
  the X adapter, and IMAP are the real providers; do not add another until all
  three are reliable and their safety tests pass.

## Independence

Herald is its own repository and does not depend on Zyquor. Shared browser and
policy code may be extracted later, but this repo must build and ship on its own.
