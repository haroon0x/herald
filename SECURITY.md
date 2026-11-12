# Security Policy

Herald handles private communications and can send mail on the user's behalf, so
security reports are taken seriously. The project is maintained on a best-effort
basis by a small team; this document describes what is supported, how to report
a problem, and what the threat model actually guarantees.

## Supported versions

| Version | Supported | Notes |
| --- | --- | --- |
| `1.x` | ✅ Yes | Security fixes land on the latest `1.x` release. |
| `0.x` | ❌ No | Pre-1.0 previews. Upgrade to `1.x`; no patches are backported. |

Only the latest `1.x` release is supported. If you are unsure whether a finding
affects you, report it anyway and include the version output of
`herald --version`.

## Dependency advisories

Dependency advisories are reviewed with `cargo deny check` (RustSec) whenever a
dependency changes; that is the gate. GitHub Dependabot is not used: its config
and PRs were removed on 2026-11-12, and vulnerability alerts were turned off, so
the two records below are the ones it found before it was removed. A finding is
either fixed or recorded here with the reason it is accepted; nothing is
silently ignored. `cargo deny check` is green as of 2026-11-12.

Fixed: `libsqlite3-sys` 0.30.1 (RUSTSEC-2026-0459 / CVE-2026-119375) was bumped
to 0.39.1 via `rusqlite` 0.41 in commit `619a488`.

Open, accepted:

| Advisory | Crate and path | Why it is accepted | Exit |
| --- | --- | --- | --- |
| GHSA-wrw7-89jp-8q8g (medium) | `glib` 0.18.5 via `apps/desktop` → `tauri` → `gtk` | Unsoundness in `glib::VariantStrIter`. The desktop shell only spawns `herald ui` and loads its page; it does not use that iterator. The fix requires gtk-rs 0.20 across the WebKitGTK stack, which no current `wry` release supports. | Next Tauri/wry bump |
| GHSA-2326-pfpj-vx3h (low) | `lexical-core` 0.7.6 via `imap` → `nom` 5 | nom 5 compiles its `lexical` float parsers, but neither `imap` nor `imap-proto` calls them (verified: no `float`/`lexical` call sites in either crate), so the affected routines are unreachable from Herald's IMAP path. The fix requires an `imap` crate upgrade. | When a newer `imap` ships |

The desktop shell is its own workspace with its own lockfile, so its advisory
does not affect the `herald` binary or the main `Cargo.lock`.

## Reporting a vulnerability

**Do not open a public issue, discussion, or pull request for a security
problem.** Public disclosure before a fix puts users at risk.

Use GitHub's private vulnerability reporting:

1. Open <https://github.com/haroon0x/herald/security/advisories/new>.
2. Describe the issue using the guidance below.
3. Submit. The report is visible only to the maintainer and GitHub's security
   tooling until an advisory is published.

If you cannot use GitHub Security Advisories, email
<haroonbmc0@gmail.com> with `[SECURITY]` in the subject. Do not include
credentials, real message contents, or third-party personal data.

### What to include

- **Affected version and platform**: `herald --version`, OS and architecture,
  install method, and the provider involved (`fake`, `maildir`, or a planned
  provider).
- **Summary**: one sentence describing the impact, for example "a Maildir
  subject can cause the planner to emit a send action without approval".
- **Reproduction**: minimal steps, configuration, and, where relevant, a
  redacted mailbox fixture. A proof of concept is helpful; do not test against
  accounts you do not own.
- **Impact**: what an attacker gains, what data or authority is exposed, and
  whether user interaction is required.
- **Evidence**: logs, stack traces, or a failing test with secrets removed.
- **Disclosure**: whether you plan to publish, and any deadline you are working
  toward.

### Response targets

These are best-effort targets, not a contractual SLA:

| Stage | Target |
| --- | --- |
| Acknowledge the report | Within 72 hours |
| Initial assessment and severity | Within 7 days |
| Fix or mitigation plan | Within 30 days for high severity; otherwise as soon as practical |
| Coordinated disclosure | 90 days after the report, or sooner by agreement |

If a fix takes longer, the maintainer will say so and agree on a disclosure
date. Reporters are credited in the advisory unless they ask to remain
anonymous. Herald does **not** run a bug bounty and cannot pay for reports.

## Scope

In scope, with the current `1.x` codebase:

- **Prompt injection** that causes an action the user did not authorize, or that
  crosses from message content into planner instructions.
- **Approval bypass**: any path that executes a send without a matching,
  unexpired approval, or that sends to a target different from the approved one.
- **Idempotency failures**: duplicate sends after an ambiguous outcome, a blind
  retry of an `uncertain` action, or a state-machine transition that skips
  approval.
- **Credential leakage**: SMTP passwords, tokens, or message bodies written to
  logs, error output, the audit log, or the database beyond the documented
  approval retention.
- **Path traversal or symlink abuse** in Maildir handling, the model directory,
  or config-relative paths.
- **Unsafe parsing**: malformed provider data or plans that cause arbitrary
  execution, panic in a security-relevant path, or a policy decision to be
  skipped.
- **Network exposure**: the managed inference sidecar reachable beyond
  `127.0.0.1`, a missing TLS check in SMTP or model downloads, or model
  artifact signature/checksum bypass.
- **Dependency vulnerabilities** that are reachable in Herald's usage.

Out of scope:

- **Auto-send.** Herald has no auto-send by design. "It sends after I approved
  it" is the intended behaviour, not a vulnerability.
- **A local attacker with the same OS user privileges** reading the SQLite
  database, config, or environment. Herald is a local-first single-user tool and
  does not claim protection from malware already running as you.
- **Provider-side compromise** of a mail service, or account security settings
  the user controls.
- **Model quality**: a wrong summary or an imperfectly chosen target that is
  still validated and approved is a product bug, not a security vulnerability.
  Report it as a normal issue.
- **Social engineering**, physical access, or attacks requiring a modified
  binary.
- **Denial of service against a user's own mailbox** by importing an
  unreasonably large mailbox.
- **Missing hardening** without a demonstrated impact, such as a report that a
  binary is not stripped.

If you are unsure whether something is in scope, report it privately and ask.

## Threat model summary

Herald's threat model is documented in full in
[docs/threat-model.md](docs/threat-model.md); the enforced boundaries are
described in [docs/architecture.md](docs/architecture.md#trust-boundaries). The
short version:

**Assets.** Message contents and metadata, SMTP credentials, the authority to
send mail as the user, the local SQLite state, and the integrity of the model
artifact.

**Adversaries.** Malicious or compromised message content (the main prompt
injection vector), a model that emits a hostile or malformed plan, malicious
provider responses, a network attacker positioned between Herald and a
configured SMTP server, and a local attacker who can already write to the user's
files.

**Boundaries and guarantees.**

1. Provider content is untrusted data. Prompts wrap it in delimited blocks, and
   the system prompt tells the model never to follow instructions found there.
2. The model has no execution authority. Its output is inert JSON that must pass
   `deny_unknown_fields` deserialization and `validate_plan` before it becomes a
   typed action; unknown providers or operations cannot be constructed.
3. Policy is applied in Rust. Sending is disabled by default, and enabling it
   still produces an approval record with the provider, resolved target, exact
   body, and body hash. Approval re-resolves the target and verifies the body
   hash immediately before sending.
4. Writes are idempotent and stateful. The storage layer refuses illegal
   transitions, and an `uncertain` send is reconciled by re-reading the
   provider, never retried blind.
5. Inference is loopback-only by default. The managed `llama-server` binds
   `127.0.0.1` with a random per-run token, and the runtime refuses a model
   whose checksum does not match the pinned manifest.
6. Secrets come from the environment by default. `init` does not write
   passwords, and logs redact them.

**Residual risks.** A local process running as the same user can read the
database and environment. A user-configurable OpenAI-compatible endpoint means
that choosing a remote endpoint sends prompts (including wrapped message
content) to that endpoint; this is explicit and documented. SMTP security
depends on the configured transport (`starttls`, `tls`, or `none`) and the
provider's own TLS certificate. Model output can be wrong even when it is
harmless; approval is the final human check.

## Safe harbor

Herald supports good-faith security research. If you make a reasonable effort to
follow this policy, avoid privacy violations, data destruction, and service
disruption, and test only against accounts and systems you own or have explicit
permission to use, the maintainers will:

- consider your research authorized under this policy;
- not pursue or support legal action against you for the research;
- work with you on coordinated disclosure and credit you if you wish.

This safe harbor does not cover activity that violates applicable law, accesses
third-party data without permission, or intentionally harms users. If in doubt,
ask before testing.

## After a report

The maintainer will confirm the affected versions, prepare a fix with a
regression test, publish a GitHub Security Advisory, and release a patched
version. Where practical, the advisory will include affected versions, impact,
workarounds, and credit. General security questions that are not vulnerabilities
can go to [Discussions](https://github.com/haroon0x/herald/discussions).
