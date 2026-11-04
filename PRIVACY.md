# Privacy

Herald is local-first by construction. It runs on your machine, stores its state
on your machine, and does not phone home. This document describes exactly what
is stored, where it lives, how long it is kept, and which network connections
the software can make. It reflects Herald `1.x`; if the data model changes, this
file changes with it.

## Summary

- **No telemetry, analytics, crash reporting, or usage tracking.** Herald does
  not collect, transmit, or sell anything.
- **No account with the Herald project.** There is no Herald cloud service.
- **No automatic update checks.** Herald never contacts GitHub on its own.
- **Local inference by default.** Message content stays on the machine unless
  you explicitly configure a remote OpenAI-compatible endpoint.
- **Minimal retention.** Read messages are stored as metadata, not full bodies.
  Bodies retained for pending approvals are deleted when the approval is
  decided or expires.

## What Herald stores

Herald keeps a SQLite database (`herald.db`) and a model directory in its data
directory, and a TOML configuration file in its config directory.

| Data | Where | Purpose | Retention |
| --- | --- | --- | --- |
| Message metadata: provider, provider message id, thread id, sender, subject, timestamp, unread flag, last seen | `herald.db` → `messages` | Deduplication and inbox/thread views | Until you delete it; records are updated in place, not versioned |
| Action records: action id, provider, operation, target description and JSON, resolved thread id, body hash, state, timestamps | `herald.db` → `actions` | Idempotency, reconciliation, and the action state machine | Until you delete the database |
| Audit events: timestamp, action id, status, short detail | `herald.db` → `audit_events` | An append-only record of what Herald did and why | Until you delete the database |
| Pending approval: provider, target, thread id, **message body**, body hash, creation and expiry times | `herald.db` → `pending_approvals` | Shows you exactly what will be sent and re-verifies it before sending | Deleted when you approve, reject, or the approval expires (72 hours by default) |
| Preferences | `herald.db` → `preferences` | Local settings that are not configuration | Until you delete the database |
| Provider health and profile references | `herald.db` → `providers` | `herald providers` and `herald doctor` | Until you delete the database |
| Configuration: provider paths, SMTP host/port/username, model backend and endpoint, policy settings | `config.toml` | Tells Herald what to connect to | Until you delete the file |
| Model weights and manifest | `<data dir>/models/` | Local inference | Until you delete the files |
| Sent copy of an approved Maildir message | Your configured `sent_path` (usually `<maildir>/.Sent/cur/`) | Proof of what was sent and of the provider's verification | Until you delete it from your mailbox |

A plaintext `password` key is accepted in `[maildir.smtp]` for compatibility.
`herald init` never writes one, `herald doctor` warns when one is present, and
the supported path is `password_env`, which names an environment variable
instead of storing the secret in the file.

Herald does **not** store full message bodies for normal reads. It stores the
metadata needed to deduplicate and display a thread. A body is written only when
an outbound action is waiting for approval, and that row is deleted as soon as
the action is approved, rejected, expires, or fails closed.

The SQLite database is **not encrypted**. It is protected by your operating
system account and file permissions. If your disk is not encrypted, enable full
disk encryption (FileVault, BitLocker, or LUKS) before using Herald with real
mail. The same applies to backups: a backup made while an approval was pending
may contain the retained body until the backup is rotated out.

## Where the data lives

Herald uses the platform's standard config and data directories. Run
`herald doctor` to print the exact paths; `--json` returns them as machine-readable
output.

| Platform | Data directory | Config directory |
| --- | --- | --- |
| Linux | `${XDG_DATA_HOME:-$HOME/.local/share}/herald` | `${XDG_CONFIG_HOME:-$HOME/.config}/herald` |
| macOS | `~/Library/Application Support/dev.Herald.herald` | `~/Library/Application Support/dev.Herald.herald` |
| Windows | `%APPDATA%\Herald\herald\data` | `%APPDATA%\Herald\herald\config` |

Environment overrides take precedence:

- `HERALD_DATA_DIR` — database and models.
- `HERALD_CONFIG` — path to the configuration file.
- `HERALD_SMTP_PASSWORD` — SMTP password, read from the environment; this is a
  credential, not stored data.
- `HERALD_MODEL_URL` and related model settings may override the download source
  in development builds.

Command-line flags take precedence over environment variables, which take
precedence over the config file, which takes precedence over defaults.

## Network connections

At **runtime**, Herald makes network connections only to the services you
configure or explicitly invoke:

1. **Your SMTP server**, and only when you approve a send through `maildir`.
   The connection uses the transport you configure (`starttls`, `tls`, or
   `none`) and includes the message body you approved.
2. **Hugging Face**, and only when you run an explicit model download such as
   `herald model download`. The request fetches the pinned GGUF artifact
   referenced by the model manifest. It sends no message content and no
   telemetry; the artifact is verified against the manifest checksum.
3. **Your OpenAI-compatible endpoint**, and only when you set
   `model.backend = "openai"` or pass the corresponding option. Prompts,
   including any message content wrapped as untrusted data, are sent to the
   endpoint you chose. This is an explicit opt-in and is the one case where
   message content leaves the machine. If the endpoint is remote, its operator
   can see the prompts.
4. **GitHub, for updates — never.** Herald has no update checker, no version
   ping, and no automatic release lookup. `install.sh` and manual downloads
   contact GitHub, but only when you run them; see below.

There are no other runtime connections. In particular, there is no analytics
endpoint, no error-reporting service, no model-routing proxy, and no Herald
account server.

**Installation-time connections.** If you install with `install.sh`, it calls
the GitHub API to resolve the latest release and downloads the archive and
`SHA256SUMS` from GitHub. If you build from source, Cargo fetches crates from
crates.io. Those tools send the usual HTTP request metadata; they do not receive
your mail, database, or configuration.

**Local inference.** By default Herald manages a `llama-server` child process
bound to `127.0.0.1` with a random per-run token. It is not reachable from your
network. The model runs on your hardware.

## What Herald does not collect

- No telemetry, event logging, feature flags, or usage counters.
- No crash reports or stack traces sent anywhere.
- No advertising identifiers or device fingerprints.
- No address book upload, contact sync, or mailbox scanning by the project.
- No message content used to train models.
- No "anonymous statistics" of any kind.

## Third parties

Herald itself has no third-party data processors. When you use a provider,
SMTP server, Hugging Face, or a model endpoint, that service's own privacy policy
applies to the data it receives. The project cannot control those services.

## Deleting your data

Everything Herald stores is a local file. Messages in your Maildir, and the Sent
copies Herald wrote there, are part of your mail store rather than Herald's
private database: deleting Herald's data does not delete your mail. To remove
everything Herald created:

1. Stop any running `herald` process.
2. Delete the data directory (database, WAL files, and downloaded models):

   ```bash
   # Linux
   rm -rf "${XDG_DATA_HOME:-$HOME/.local/share}/herald"

   # macOS
   rm -rf "$HOME/Library/Application Support/dev.Herald.herald"
   ```

   On Windows (PowerShell):

   ```powershell
   Remove-Item -Recurse -Force "$env:APPDATA\Herald\herald"
   ```

   If you set `HERALD_DATA_DIR` or `HERALD_CONFIG`, delete those paths instead.

3. Delete the config directory, or just the file, to remove SMTP settings and
   endpoint configuration:

   ```bash
   # Linux
   rm -rf "${XDG_CONFIG_HOME:-$HOME/.config}/herald"

   # macOS (same directory as the data above)
   rm -f "$HOME/Library/Application Support/dev.Herald.herald/config.toml"
   ```

4. Unset credentials in your shell and remove them from your shell profile:
   `HERALD_SMTP_PASSWORD`, any model endpoint API key, and any `HERALD_MODEL_URL`.
5. Remove the binary if you no longer want Herald:
   `rm "${HERALD_INSTALL_DIR:-$HOME/.local/bin}/herald"`.
6. Check backups and snapshots. Time Machine, File History, cloud backup, and
   filesystem snapshots can retain copies of the database after you delete the
   live file. Delete those snapshots according to your backup tool's
   instructions.

Deleting the database removes message metadata, action records, audit events,
preferences, and any pending approval bodies. There is no server-side copy to
delete because there is no server.

## Children and sensitive data

Herald is a general-purpose local tool, not a service directed at children, and
the project does not knowingly collect data from anyone. Because Herald can
process email, treat its data directory like your mailbox: restrict file
permissions and avoid sharing the database or config file in bug reports.

## Changes to this policy

This policy is versioned with the repository. Material changes will be described
in [CHANGELOG.md](CHANGELOG.md). If a future feature introduces a network call or
a new store of personal data, this document will be updated before the feature
ships.

## Questions

Privacy questions that are not security vulnerabilities can go to
[Discussions](https://github.com/haroon0x/herald/discussions). To report a data
exposure or credential leak, use the private process in [SECURITY.md](SECURITY.md).

_Last updated: 2026-11-04._
