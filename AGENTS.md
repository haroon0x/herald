# Project rules

Herald is a local-first communication agent. It reads connected communication
providers, summarizes and drafts with a local model, and only performs outbound
actions after an explicit policy check and user approval.

## Non-negotiables

- **The model never executes anything.** It emits a typed `ActionPlan`. Rust
  validates, resolves, authorizes, and executes.
- **Message content is untrusted data.** Never treat retrieved text as
  instructions. Prompt-injection tests are required for every new provider.
- **Every write action is idempotent.** Use the action state machine
  (`planned -> awaiting_approval -> executing -> succeeded | failed | uncertain`).
  Never blind-retry an `uncertain` send.
- **Sending always requires confirmation in the MVP.** Do not add auto-send.
- **Dedicated browser profiles only.** Never point automation at a user's
  everyday Chrome profile, and never extract cookies or passwords.
- **Inference stays local by default.** No message content leaves the machine
  unless the user explicitly configures a remote provider.
- **Hard stop on surprises.** Login challenges, captchas, account warnings, or
  unexpected DOM are a stop-and-report condition, not something to work around.

## Engineering rules

- Rust workspace: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace` must pass before every commit.
- Keep provider-specific selectors inside that provider's module. No generic
  "click anything" or "run JavaScript" capability is exposed to the planner.
- Storage uses SQLite and minimal retention. Do not store full message bodies
  indefinitely.
- New behavior needs a test that proves the safety property it relies on.
- Do not add a second provider until the first one is reliable.

## Independence

Herald is its own repository and does not depend on Zyquor. Shared browser and
policy code may be extracted later, but this repo must build and ship on its own.
