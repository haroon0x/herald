# Support

Herald is maintained on a best-effort basis by a small team. There is no
commercial support contract, no guaranteed response time, and no private support
email (except for security reports).

## Where to go

| I need... | Go to |
| --- | --- |
| Help using Herald, configuration questions, ideas | [GitHub Discussions](https://github.com/haroon0x/herald/discussions) |
| To report a reproducible bug | [Bug report](https://github.com/haroon0x/herald/issues/new?template=bug_report.yml) |
| To propose a feature or provider | [Feature request](https://github.com/haroon0x/herald/issues/new?template=feature_request.yml) or [provider request](https://github.com/haroon0x/herald/issues/new?template=provider_request.yml) |
| To report a vulnerability | [Private security advisory](https://github.com/haroon0x/herald/security/advisories/new) — never a public issue |

Read [SECURITY.md](SECURITY.md) before reporting anything that could expose
credentials, message contents, or an approval bypass.

## Before you ask

Most problems are answered by these steps:

```bash
herald --version
herald doctor
herald config path
herald config list
```

Include the output of `herald --version` and `herald doctor` in your question,
with secrets removed. `herald doctor --json` is easier to paste and redact.

Also check:

- [README.md](README.md) for the quickstart and the current status.
- [docs/configuration.md](docs/configuration.md) for every config key and its
  precedence.
- [docs/providers.md](docs/providers.md) for Maildir and SMTP setup.
- [docs/architecture.md](docs/architecture.md) for how the pieces fit together.
- [CONTRIBUTING.md](CONTRIBUTING.md) if you want to fix the problem yourself.

## Supported versions

The latest `1.x` release is supported. Pre-1.0 `0.x` previews are unsupported;
upgrade to `1.x` before reporting a problem. If you built from a commit rather
than a release, include the commit hash.

## What we can help with

- Installing Herald from a release or from source.
- Configuring Maildir, SMTP, and local model backends.
- Understanding an approval, action state, or audit entry.
- Reproducing a bug in the CLI, storage, or provider adapters.
- Reviewing a proposed provider or safety-model change.

## What we cannot help with

- Debugging your mail server, DNS, TLS certificates, or provider account.
- Recovering mail that a sync tool deleted or moved outside Herald.
- Tuning a third-party model or inference server beyond Herald's configuration.
- Private one-to-one support or consulting.
- Security reports in public issues; use the private advisory form instead.

## Response expectations

Questions and issues are triaged in batches, usually within a few business days.
Security reports follow the targets in [SECURITY.md](SECURITY.md) (72-hour
acknowledgement, best effort). A slow reply is not a decision; if an issue is
important, add the context that makes it reproducible rather than pinging
repeatedly.

## Contributing a fix

If you have found a bug and can reproduce it, a pull request is welcome. Read
[CONTRIBUTING.md](CONTRIBUTING.md) first: safety-sensitive paths need tests, and
all changes must pass formatting, Clippy, tests, and documentation gates.
