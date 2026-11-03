# Herald

Local-first communication agent. Herald reads connected communication providers,
summarizes and drafts with a local model, and only sends after an explicit policy
check and user approval.

```text
instruction -> plan (rules now, local model next) -> typed action
            -> policy check -> provider adapter -> verified result
            -> SQLite state + audit log
```

Status: **Phase 0/1 skeleton with a working local read/draft loop.** The agent
shell, typed action model, policy engine, idempotent state machine, storage,
provider boundary, and managed inference runtime exist. `fake` is the only
executable adapter: reads run through it, persist deduped message metadata, and
transition their action records; drafts are prepared locally. No adapter talks
to a real service yet, no model is downloaded yet, and nothing can send
anything. `--provider gmail` and `--provider x` are plan-only until Phase 2.

## Safety model

- The model never executes anything. It emits one JSON `ActionPlan`; Rust
  validates it into typed actions with `deny_unknown_fields` and a closed
  provider/operation enum.
- Message content is wrapped as untrusted data in prompts and is never read as
  instructions.
- Sending is disabled by default. Even when enabled, every send produces an
  approval request with provider, target, and exact body before execution.
- Write actions live in an explicit state machine
  (`planned -> awaiting_approval -> executing -> succeeded | failed | uncertain`).
  The storage layer rejects illegal transitions; `uncertain` sends are
  reconciled, never retried blind.
- Inference runs in an application-managed `llama-server` child bound to
  `127.0.0.1` only, with a random per-run API token. There is no config field
  that can expose it to the network.

## Repository layout

```text
crates/herald-core       typed actions, validation, policy, state machine, prompts
crates/herald-provider   capability trait + deterministic fake provider
crates/herald-storage    SQLite migrations, message dedupe, action records, audit
crates/herald-runtime    model manifest, checksums, llama-server sidecar client
apps/agent               herald CLI (doctor, plan, providers, inbox, thread, history,
                         eval, execution loop)
docs/                    architecture, roadmap, decision records, status report
experiments/wsp/         weight-space programming pilot (small-model harness)
models/manifest.json     pinned model identity (checksum filled after the spike)
```

## Quickstart

```bash
rustup show   # stable toolchain
cargo test --workspace

export HERALD_DATA_DIR=/tmp/opencode/herald-data

herald doctor
herald plan "Check whether I have anything important."
herald plan "Summarize the latest email from the founder."
herald plan "Draft a reply saying I'm interested."
herald plan 'Send exactly: "Thanks, I will review this."' --enable-send
herald eval --engine rules
herald history
```

`plan` maps the instruction deterministically (no inference), validates it,
applies policy, and executes allowed reads against the fake provider. Message
metadata is deduped in SQLite, action records move through the state machine,
and every step is audited. Drafts stay local. `--enable-send` only demonstrates
the approval path; nothing is sent.

`eval` scores the checked-in command set (`eval/commands.json`, 42 cases
including prompt-injection and ambiguity cases) against the rules engine or a
local model.

## Development

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

All three must pass before every commit. See `AGENTS.md` for the rules the code
is expected to enforce, not just document.

## License

AGPL-3.0 for the open source core, pending final licensing decision (see
`docs/decisions/0002-license-and-product.md`).
