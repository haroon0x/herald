# Herald documentation

Start with the [README](../README.md) for the pitch and the demo. Come here for
the contract, the reasoning, and the failure modes.

## Reading order

| # | Document | What it answers |
| --- | --- | --- |
| 1 | [architecture.md](architecture.md) | How the system is built: crates, data flow, the type ladder, the state machine, trust boundaries. |
| 2 | [v1-design.md](v1-design.md) | The v1.0 contract: types, provider port, storage schema, CLI surface. |
| 3 | [providers.md](providers.md) | What `fake` and `maildir` do, how threading and Sent verification work, and what a new provider must satisfy. |
| 4 | [configuration.md](configuration.md) | Every key, path, environment variable, and precedence rule. |
| 5 | [cli.md](cli.md) | Every command, its options, human and JSON output, exit codes, and `jq` recipes. |
| 6 | [threat-model.md](threat-model.md) | Assets, adversaries, attack paths, approval UX risks, and the test that proves each property. |
| 7 | [faq.md](faq.md) | The short answers: auto-send, local models, offline use, deletion, GUIs, Ollama. |
| 8 | [roadmap.md](roadmap.md) | What shipped, what comes next, and the ladder for model authority. |
| 9 | [decisions/](decisions/) | Why each load-bearing choice was made. |
| 10 | [status-2026-11-03.md](status-2026-11-03.md) | A historical snapshot of what was measured on that date. Not current. |

## By task

**I want to try it.** [README quickstart](../README.md#quickstart), then
[configuration.md](configuration.md) for the Maildir block.

**I want to connect real mail.** [providers.md](providers.md#maildir) and
[configuration.md](configuration.md#example-configs).

**I want to automate it.** [cli.md](cli.md#scripting) and the JSON output
contract.

**I want to audit the safety claims.** [threat-model.md](threat-model.md) and
[architecture.md](architecture.md#trust-boundaries).

**I want to add a provider.** [providers.md](providers.md#adding-a-provider).

**I want to know what the model is allowed to do.** [roadmap.md](roadmap.md#the-model-ladder).

## Decision records

| ADR | Decision | Status |
| --- | --- | --- |
| [0001](decisions/0001-repo-boundary.md) | Independent repository, late convergence gate | Accepted |
| [0002](decisions/0002-license-and-product.md) | AGPL-3.0-or-later; Pro tier not in v1 | Accepted |
| [0003](decisions/0003-inference-engine.md) | `llama.cpp` sidecar, not vLLM | Accepted |
| [0004](decisions/0004-email-provider-maildir-smtp.md) | Maildir plus SMTP for v1; no browser automation | Accepted |
| [0005](decisions/0005-approval-queue-and-idempotency.md) | Durable approvals, client token, no blind retry | Accepted |
| [0006](decisions/0006-inference-backends.md) | Managed sidecar by default, OpenAI-compatible escape hatch | Accepted |
| [0007](decisions/0007-browser-automation.md) | No browser automation in v1; `chromiumoxide` is the documented alternative | Accepted |

## Project files

- [SECURITY.md](../SECURITY.md): how to report a vulnerability.
- [CONTRIBUTING.md](../CONTRIBUTING.md): build, test, and review expectations.
- [PRIVACY.md](../PRIVACY.md): what is stored, what leaves the machine.
- [CHANGELOG.md](../CHANGELOG.md): release history.
- [AGENTS.md](../AGENTS.md): the rules the code is expected to enforce.

## Assets

The diagrams and brand files referenced by these documents:

| File | Used by |
| --- | --- |
| `assets/logo.svg` | README header |
| `assets/banner.svg` | Project pages and release notes |
| `assets/architecture.svg` | [architecture.md](architecture.md) |
| `assets/demo.svg` | Demo walkthroughs |
| `assets/mascot.svg` | The messenger owl, used in project personality and docs |
| `assets/social-preview.svg` | Repository social preview |

The Mermaid diagrams in the docs are the source of truth. The SVG versions are
renderings for places that cannot render Mermaid.
