# Contributing to Herald

Herald is a local-first communication agent with a deliberately narrow safety
model. Contributions are welcome, especially tests, provider reliability work,
and documentation. This guide explains how to build the project, what the
non-negotiable rules are, and what a good pull request looks like.

By participating you agree to follow the [Code of Conduct](CODE_OF_CONDUCT.md).
For security issues, stop here and read [SECURITY.md](SECURITY.md): never open a
public issue with an exploit.

## Before you start

- Search [issues](https://github.com/haroon0x/herald/issues) and
  [Discussions](https://github.com/haroon0x/herald/discussions) first.
- Bug reports use the bug template; feature ideas use the feature template;
  new communication providers use the provider template. All three ask for the
  context needed to make a decision.
- Open an issue before a large change. Herald prefers small, reviewable patches
  over a surprise rewrite, and a design discussion is cheaper for everyone than
  a declined PR.
- Read [AGENTS.md](AGENTS.md) and [docs/architecture.md](docs/architecture.md)
  before touching validation, policy, approvals, storage, or providers.

## Development setup

Herald needs Rust **1.98 or newer** (edition 2024). The pinned toolchain is
declared in `rust-toolchain.toml`.

```bash
git clone https://github.com/haroon0x/herald.git
cd herald

rustup show                 # installs/uses the pinned stable toolchain
cargo build --workspace
cargo test --workspace --all-targets
```

The test suite uses temporary directories and the deterministic `fake`
provider. It must not need network access, a real mailbox, an SMTP server, or a
downloaded model. If you need to run the CLI by hand, keep its state out of your
real data directory:

```bash
export HERALD_DATA_DIR=/tmp/herald-dev/data
export HERALD_CONFIG=/tmp/herald-dev/config.toml
cargo run -p herald-agent --bin herald -- doctor
```

[just](https://github.com/casey/just) is optional but convenient:

```bash
just            # list recipes
just check      # fmt + clippy + test + docs
```

## Repository map

| Path | Responsibility |
| --- | --- |
| `crates/herald-core` | Typed actions, plan validation, policy decisions, action state machine, prompt trust boundary. No I/O. |
| `crates/herald-config` | TOML configuration, precedence (CLI > env > file > default), secret redaction. |
| `crates/herald-provider` | `CommunicationProvider` trait and adapters (`fake`, `maildir`). The only code that knows a provider's wire format. |
| `crates/herald-storage` | SQLite schema, message dedupe, action records, pending approvals, append-only audit log. |
| `crates/herald-runtime` | Model manifest, checksums, managed `llama-server` sidecar, OpenAI-compatible client. |
| `apps/agent` | The `herald` CLI: `init`, `doctor`, `ask`, `plan`, `approvals`, `approve`, `reject`, `reconcile`, `eval`, and friends. |
| `docs/` | Architecture, roadmap, design notes, decision records. |
| `eval/` | Checked-in command set used to score the planner. |
| `tests/` | Cross-crate integration tests. |

## Non-negotiable safety invariants

These come from [AGENTS.md](AGENTS.md). They are enforced by code and tests, and
a change that weakens one will not be merged.

1. **The model never executes anything.** It emits a typed `ActionPlan`; Rust
   validates, resolves, authorizes, and executes. Do not add a tool registry, a
   "run this" escape hatch, or model-generated shell/HTTP calls.
2. **Message content is untrusted data.** Retrieved text is wrapped as data in
   prompts and never interpreted as instructions. Every new input path needs a
   prompt-injection test.
3. **Every write is idempotent.** Writes move through
   `planned -> awaiting_approval -> executing -> succeeded | failed | uncertain`
   (plus `cancelled`). Never blind-retry an `uncertain` send; reconcile it.
4. **Sending requires explicit approval every time.** No auto-send, not even in
   demos or tests. The default policy is `allow_send = false`.
5. **Inference is local by default.** No message content leaves the machine
   unless the user explicitly configures a remote or OpenAI-compatible endpoint.
6. **Dedicated browser profiles only.** Never point automation at a user's
   everyday browser profile, and never extract cookies or passwords.
7. **Hard stop on surprises.** Login challenges, captchas, account warnings, or
   unexpected page structure stop and report; they are not worked around.
8. **Storage keeps minimal data.** Do not retain full message bodies
   indefinitely. Pending approval bodies are deleted on decision or expiry.

If a change cannot preserve one of these, it belongs in a design discussion, not
a pull request.

## Quality gates

Every pull request must pass the same gates CI runs:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo deny check
```

- `cargo deny` requires `cargo install cargo-deny` and checks advisories,
  licenses, duplicate/wildcard dependencies, and sources against `deny.toml`.
- `cargo audit` (or the weekly audit workflow) covers RustSec advisories.
- CI runs the test suite on Linux, macOS, and Windows, plus MSRV 1.98.0.
- Formatting is configured in `rustfmt.toml`; do not reformat unrelated files.

### Test requirements

New behaviour needs a test that proves the property it relies on, not just a
happy-path assertion.

- **Safety changes** (validation, policy, approval, state machine) need a test
  that attempts the unsafe action and asserts it is rejected.
- **Provider changes** need contract tests, a prompt-injection case, and an
  idempotency/reconciliation case.
- **Storage changes** need a migration test and a test for illegal transitions
  being refused.
- **CLI changes** should cover both human output and the `--json` envelope.
- Tests must be deterministic and offline. Use temporary directories, not the
  developer's home directory. Do not sleep to fix a race; fix the race.

## Adding a provider

Provider support is a safety boundary, not a plugin. Open a provider request
first so the design can be reviewed before implementation.

1. **Discuss the design.** Use the provider issue template and answer the safety
   questions: untrusted content, credentials, approval semantics, failure modes,
   and terms of service.
2. **Add the identity.** Add or update the `ProviderId` variant in
   `herald-core`, including its `Display`/`FromStr` behaviour and the
   `is_implemented` signal the CLI uses to avoid pretending a planned provider
   works.
3. **Implement the trait.** Implement `CommunicationProvider` in
   `herald-provider`: `health`, `list_recent`, `get_thread`, `prepare_reply`,
   `send_reply`, and `find_sent`. There is no generic "click", "run JavaScript",
   or "call this URL" method, and there must not be one.
4. **Fail closed.** Ambiguous targets, missing credentials, and unexpected
   responses are errors. `send_reply` must be idempotent on `client_token` and
   `find_sent` must be able to resolve an `uncertain` action without sending
   again.
5. **Keep secrets out of files and logs.** Read credentials from the
   environment (or an OS keychain). A plaintext password in config may be
   accepted for compatibility, but `doctor` must warn and `init` must never
   write one.
6. **Add tests.** Cover the trait contract, prompt injection, duplicate sends,
   timeout reconciliation, malformed provider data, and the hard-stop cases.
7. **Update documentation.** Add the provider to the README provider table,
   `docs/architecture.md`, and the config reference in `docs/v1-design.md`.
   Provider-specific selectors stay inside the provider module.

Do not add a second real provider until the first one is reliable. The `fake`
provider and `maildir` are the reference implementations.

## Commit style

Herald uses [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/):

```text
<type>(<optional scope>): <imperative summary>

<why the change is needed>

Refs: #123
```

Common types: `feat`, `fix`, `docs`, `test`, `refactor`, `perf`, `build`, `ci`,
`chore`, `revert`. Useful scopes: `core`, `config`, `provider`, `storage`,
`runtime`, `cli`, `ci`, `release`, `docs`.

Examples:

```text
feat(maildir): copy sent messages to the Sent mailbox
fix(storage): refuse succeeded -> executing transitions
test(core): cover prompt injection through a Maildir subject
ci: cache cargo builds across the test matrix
```

Rules:

- Subject in the imperative mood, lowercase, no trailing period, ideally under
  72 characters.
- The body explains *why*, not a restatement of the diff.
- Reference issues with `Refs: #123` or `Closes: #123`.
- Keep commits scoped. Squash fixup commits before requesting review.
- Never commit credentials, tokens, real message contents, generated models, or
  `target/` output.

## Pull requests

1. Branch from `master` and keep one logical change per PR.
2. Fill in the PR template, including the safety checklist. If a checkbox does
   not apply, say why rather than leaving it ambiguous.
3. Link the issue the PR addresses.
4. Make sure the full quality gate passes locally; CI will run it again.
5. Add screenshots or terminal transcripts for user-visible changes, with
   secrets and personal data removed.
6. Keep the PR description current if the approach changes during review.

Draft PRs are welcome for early feedback. Maintainers may ask for a smaller
scope, additional tests, or a design note; that is normal, not a rejection.

## Review expectations

- A maintainer reviews every PR. Security-sensitive paths listed in
  [CODEOWNERS](.github/CODEOWNERS) require maintainer approval: `herald-core`,
  `herald-provider`, `herald-storage`, and the workflow files.
- Reviews focus on correctness, safety invariants, tests, and cross-platform
  behaviour. Style is handled by `rustfmt` and `clippy`.
- Herald is maintained on a best-effort basis. You should get a response within
  a few business days, but there is no guaranteed SLA.
- If a PR goes stale, a polite ping on the issue is fine. Maintainers will close
  PRs that have been inactive for several months and cannot be revived.
- Do not force-push over review comments. Rebase when asked; otherwise add
  commits so the discussion stays reviewable.

## Good first issues

Look for issues labelled **`good first issue`** and **`help wanted`**. Good
starting points that do not require deep safety work:

- Improve a CLI error message so it names the file or configuration key at
  fault.
- Add a test for config precedence (CLI flag > environment > file > default).
- Add an `eval/commands.json` case for a phrasing the rules router mishandles.
- Improve `install.sh` for a platform or shell it does not yet cover (for
  example musl-based Linux).
- Fix documentation inaccuracies or broken links; docs changes follow the same
  PR process.
- Add shell-completion or `--json` output tests for an existing command.

To claim one, comment on the issue with a short plan. Wait for a maintainer to
assign it, then open a draft PR when you have the shape of the change. Ask
questions early; nobody is expected to guess at the safety model.

## Licensing

Herald is licensed under **AGPL-3.0-or-later**; see [LICENSE](LICENSE). There is
no CLA and no DCO sign-off requirement. By submitting a pull request you confirm
that you have the right to submit the work and that it may be distributed under
the same license as the project (inbound = outbound). If your employer has
rights to your work, make sure you have permission before contributing.

## Questions

Usage questions and open-ended ideas belong in
[Discussions](https://github.com/haroon0x/herald/discussions). Bugs and concrete
feature proposals belong in issues. Security reports belong in the private
advisory form described in [SECURITY.md](SECURITY.md).
