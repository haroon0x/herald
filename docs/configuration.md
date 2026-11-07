# Configuration

Herald reads one TOML file and a fixed set of environment overrides. There is
no account setup, no cloud project, and no OAuth client to register.

## Where the file lives

| Platform | Typical config path | Typical data path |
| --- | --- | --- |
| Linux | `~/.config/herald/config.toml` | `~/.local/share/herald/` |
| macOS | `~/Library/Application Support/dev.Herald.herald/config.toml` | `~/Library/Application Support/dev.Herald.herald/` |
| Windows | `%APPDATA%\Herald\herald\config\config.toml` | `%APPDATA%\Herald\herald\data\` |

The table is a guide. `herald config path` prints the value this build will
actually use, and `herald doctor` prints the paths it resolved.

Config file resolution:

1. `--config <path>`. The file must exist.
2. `HERALD_CONFIG`. The file must exist.
3. The platform config directory, `config.toml`. Used only if it exists;
   otherwise built-in defaults apply.

Data directory resolution:

1. `--data-dir <path>`.
2. `HERALD_DATA_DIR`. An empty value is an error.
3. The platform data directory.

The data directory is created with mode `0700` on Unix and holds `herald.db`
and `models/`.

## Precedence

Highest first:

| Rank | Source | Example |
| --- | --- | --- |
| 1 | Command-line flag | `--provider maildir`, `--enable-send`, `--engine model` |
| 2 | Environment variable | `HERALD_ALLOW_SEND=true`, `HERALD_SMTP_HOST=...` |
| 3 | `config.toml` | `default_provider = "maildir"` |
| 4 | Built-in default | `allow_send = false` |

Environment overrides are applied after the file is parsed and before validation,
so an environment value that produces an invalid configuration fails at startup
instead of being ignored.

`--enable-send` is a per-command override of `policy.allow_send`. It enables the
approval path for that invocation. It never turns on auto-send, because no such
setting exists.

`--provider` overrides `default_provider` for one command. `--engine` selects
`rules`, `auto`, or `model`.

## `herald init`

```console
$ herald init
Herald 1.0.0
data:       /home/you/.local/share/herald
database:   /home/you/.local/share/herald/herald.db
config:     /home/you/.config/herald/config.toml (written)

Nothing is connected yet. Try the demo inbox:
  herald ask "check what is unread"
  herald ask 'send exactly: "Thanks, I will review this."' --enable-send

To read real mail, edit the [maildir] section in the config, then run:
  herald doctor
```

`init` creates the data directory and database, writes the annotated starter
config with mode `0600`, and appends an `init` audit event. It never writes a
password, downloads a model, or contacts the network. It refuses to overwrite an
existing config unless `--force` is passed.

`herald config init [--force]` writes the same template without creating the
database. Both commands work even when the existing config is invalid.

## Inspecting configuration

```console
$ herald config path
/home/you/.config/herald/config.toml

$ herald config get default_provider
maildir

$ herald config get maildir.smtp.password_env
HERALD_SMTP_PASSWORD

$ herald config list
default_provider = maildir
maildir.from = Me <me@example.com>
maildir.path = /home/you/Mail
maildir.sent_path = /home/you/Mail/.Sent
maildir.smtp.host = smtp.example.com
maildir.smtp.password_env = HERALD_SMTP_PASSWORD
maildir.smtp.port = 587
maildir.smtp.security = starttls
maildir.smtp.username = me@example.com
model.backend = none
model.context_size = 4096
model.threads = 4
policy.allow_send = false
policy.approval_ttl_hours = 72
policy.max_body_chars = 8000
```

`get` prints the resolved value only, after defaults and overrides. A secret is
never printed: there is no `password` key in the flattened output, and the
value of `password_env` is the name of a variable, not the secret. `config list
--json` returns `{"config": {key: value}}`.

## Keys

Unknown keys are rejected. `deny_unknown_fields` is set on every table, so a
typo such as `auto_send = true` fails at load instead of being ignored.

### Top level

| Key | Type | Default | Example | Notes |
| --- | --- | --- | --- | --- |
| `default_provider` | string | `"fake"` | `"maildir"` | One of `fake`, `maildir`, `gmail`, `x`. A planned provider cannot be the default: validation rejects it. `x` requires an `[x]` section. |

### `[policy]`

| Key | Type | Default | Example | Notes |
| --- | --- | --- | --- | --- |
| `allow_send` | boolean | `false` | `true` | Enables the approval path for sends. Every send still needs `herald approve`. |
| `max_body_chars` | integer | `8000` | `4000` | 1 to 1,000,000. Over the limit is a policy rejection, not a truncation. |
| `approval_ttl_hours` | integer | `72` | `24` | 1 to 720. Expired approvals cannot be approved and their bodies are deleted. |

### `[maildir]`

The whole table is optional. When it is present, `path` is required. Without
`[maildir.smtp]`, the provider is read-only and `send_reply` returns
`provider_not_configured`.

| Key | Type | Default | Example | Notes |
| --- | --- | --- | --- | --- |
| `path` | path | none | `"~/Mail"` | Maildir root containing `new/` and `cur/`. `~` is expanded. |
| `sent_path` | path | `<path>/.Sent` | `"~/.Sent"` | Where the verified copy is written. Created on first send. |
| `from` | string | none | `"Me <me@example.com>"` | Envelope and `From` header. Required for sending; must contain `@` and no line breaks. |

### `[maildir.smtp]`

| Key | Type | Default | Example | Notes |
| --- | --- | --- | --- | --- |
| `host` | string | none | `"smtp.example.com"` | Submission host. Required when the table exists. No spaces or line breaks. |
| `port` | integer | `587` | `465` | Submission port. Must not be zero. |
| `username` | string | none | `"me@example.com"` | SMTP AUTH username. |
| `password` | string | none | (avoid) | Plaintext password. `doctor` warns; `init` never writes one. |
| `password_env` | string | none | `"HERALD_SMTP_PASSWORD"` | Name of the environment variable holding the password. |
| `security` | string | `"starttls"` | `"tls"` | One of `starttls`, `tls`, `none`. `none` is for a loopback relay. |

If `username` is set, one of `password` or `password_env` must be set. A
configured `password_env` that is missing from the environment is an error at
provider construction, and the Maildir falls back to read-only with a warning.
It is never a silent unauthenticated send.

### `[imap]`

Read-only Gmail IMAP. App passwords only. See
[providers.md](providers.md#imap-read-only-gmail) and
[ADR 0012](decisions/0012-imap-provider.md).

| Key | Type | Default | Example | Notes |
| --- | --- | --- | --- | --- |
| `host` | string | `"imap.gmail.com"` | `"imap.example.com"` | Hostname without spaces. |
| `port` | integer | `993` | `143` | Must not be zero. With `use_tls = false` use the plain IMAP port. |
| `username` | string | none | `"me@gmail.com"` | Login name. Requires `password` or `password_env`. |
| `password` | string | none | (avoid) | Plaintext password. `doctor` warns; `init` never writes one. |
| `password_env` | string | none | `"HERALD_IMAP_PASSWORD"` | Name of the environment variable holding the password. |
| `folder` | string | `"INBOX"` | `"Archive"` | Mailbox to read. Gmail label names work as folder names. |
| `use_tls` | boolean | `true` | `false` | Default on. Only disable for a local test server. |
| `mark_read_on_open` | boolean | `true` | `false` | Opening a thread sets `\Seen` on the server. Set `false` for side-effect-free reads. |

If `username` is set, one of `password` or `password_env` must be set. A
configured `password_env` that is missing from the environment disables the
provider with a warning at startup, like a mailbox without SMTP. It is never
a silent unauthenticated login attempt.

When `backend = "opencode"` and a managed GGUF is present, the GGUF is a
lazy fallback: the sidecar starts only if the provider fails. The managed
model is never started just to idle.

### `[x]`

The whole table is optional. When it is present, the registry builds the X
browser adapter. The profile is dedicated and must not be an everyday Chrome
profile. See [providers.md](providers.md#x-browser-adapter) and
[ADR 0008](decisions/0008-x-browser-adapter.md).

| Key | Type | Default | Example | Notes |
| --- | --- | --- | --- | --- |
| `profile_dir` | path | `<data-dir>/profiles/x` | `"~/.local/share/herald/profiles/x"` | Dedicated Chrome profile. `~` is expanded. A path that looks like a shared browser profile is refused before launch. |
| `handle` | string | none | `"@you"` | Display handle. Required for writes: verification needs it to attribute the posted message. |
| `headless` | boolean | `true` | `false` | Normal commands run headless. `herald x login` overrides this and always shows a window. |
| `action_delay_ms` | integer | `1800` | `2500` | Pause after opening a page, clicking, or submitting. |
| `typing_delay_ms` | integer | `20` | `35` | Per-character delay while typing into a composer. |
| `close_after` | boolean | `true` | `false` | Close the browser at the end of each operation. |
| `read_only` | boolean | `false` | `true` | Refuse every X write, including approved ones; reads keep working. Default is `false`: writes are allowed but still need approval. |

### `[model]`

| Key | Type | Default | Example | Notes |
| --- | --- | --- | --- | --- |
| `backend` | string | `"none"` | `"managed"` | One of `none`, `managed`, `openai`, `opencode`. Every backend is used for classification, reply drafting, and summaries. |
| `endpoint` | string | none | `"http://127.0.0.1:11434/v1"` | Required for `openai`. Must be `http://` or `https://`. |
| `model_name` | string | none | `"qwen3.5:0.8b"` | Required for `openai`; for `opencode` it is the model id (default `opencode/glm-5.3-flash-free`). |
| `manifest` | path | `<data-dir>/models/manifest.json` | `"~/models/qwen.json"` | Used by `managed`. Relative paths resolve against the data directory. |
| `api_key_env` | string | none | `"HERALD_MODEL_API_KEY"` | Optional bearer token for `openai`. Read from the environment. |
| `context_size` | integer | `4096` | `8192` | Used by `managed`. Minimum 512. |
| `threads` | integer | `4` | `8` | Used by `managed`. Minimum 1. |

The model is optional. With `backend = "none"` the rules router still handles
the common commands, and `--engine model` fails with `model_not_configured`
instead of silently falling back.

## Environment variables

Overrides applied to the configuration:

| Variable | Overrides | Example |
| --- | --- | --- |
| `HERALD_PROVIDER` | `default_provider` | `maildir` |
| `HERALD_ALLOW_SEND` | `policy.allow_send` | `true`, `1`, `yes`, `on` |
| `HERALD_IMAP_HOST` | `imap.host` | `imap.gmail.com` |
| `HERALD_IMAP_PORT` | `imap.port` | `993` |
| `HERALD_IMAP_USERNAME` | `imap.username` | `me@gmail.com` |
| `HERALD_IMAP_FOLDER` | `imap.folder` | `INBOX` |
| `HERALD_IMAP_TLS` | `imap.use_tls` | `true` |
| `HERALD_IMAP_MARK_READ_ON_OPEN` | `imap.mark_read_on_open` | `true` |
| `HERALD_X_PROFILE_DIR` | `x.profile_dir` | `/home/you/.local/share/herald/profiles/x` |
| `HERALD_X_HANDLE` | `x.handle` | `@you` |
| `HERALD_X_HEADLESS` | `x.headless` | `true` |
| `HERALD_X_ACTION_DELAY_MS` | `x.action_delay_ms` | `1800` |
| `HERALD_X_TYPING_DELAY_MS` | `x.typing_delay_ms` | `20` |
| `HERALD_X_CLOSE_AFTER` | `x.close_after` | `true` |
| `HERALD_X_READ_ONLY` | `x.read_only` | `true` |
| `HERALD_MAILDIR_PATH` | `maildir.path` | `/home/you/Mail` |
| `HERALD_SMTP_HOST` | `maildir.smtp.host` | `smtp.example.com` |
| `HERALD_SMTP_PORT` | `maildir.smtp.port` | `587` |
| `HERALD_SMTP_USERNAME` | `maildir.smtp.username` | `me@example.com` |
| `HERALD_SMTP_SECURITY` | `maildir.smtp.security` | `starttls` |
| `HERALD_MODEL_BACKEND` | `model.backend` | `openai` |
| `HERALD_MODEL_ENDPOINT` | `model.endpoint` | `http://127.0.0.1:11434/v1` |
| `HERALD_MODEL_NAME` | `model.model_name` | `qwen3.5:0.8b` |

Process and path variables:

| Variable | Used for | Default | Notes |
| --- | --- | --- | --- |
| `HERALD_DATA_DIR` | Data directory | Platform data dir | Holds `herald.db` and `models/`. |
| `HERALD_CONFIG` | Config file path | Platform config dir | Must exist when set. |
| `HERALD_SMTP_PASSWORD` | SMTP password | unset | The default name referenced by `password_env`. Any variable name works. |
| `HERALD_LLAMA_SERVER` | Path to `llama-server` | `llama-server` on `PATH` | Used by the managed backend. |
| `HERALD_OPENCODE_PATH` | Path to `opencode` | `opencode` on `PATH` | Used by the `opencode` backend. |
| `CHROME` | Path to Chrome/Chromium | autodetected | Used by the X adapter. |
| `HERALD_MODEL_URL` | Download base URL | manifest URL | For an internal mirror. The SHA-256 check still applies. |
| `HERALD_MODEL_API_KEY` | Bearer token | unset | Only when `model.api_key_env` names it. |
| `NO_COLOR` | Disable ANSI color | unset | Same as `--no-color`. |
| `HERALD_LOG` | Log filter | `warn` | The `--log` flag takes precedence. |

Logging examples: `--log info`, `--log herald_agent=debug`, or
`HERALD_LOG=info herald ...`. Logs go to stderr and never include bodies or
secrets.

## Secrets

Rules, in order of preference:

1. Put the password in an environment variable and reference its name with
   `password_env`. This is the documented path.
2. Use a secret manager that exports the variable before `herald` runs.
3. Do not put the password in `config.toml`. If you do, `chmod 600` the file and
   expect a `doctor` warning.

Herald never logs a secret, never includes one in an error message, never writes
one to the database, and never passes one as a command-line argument (where it
would appear in the process list). The `Debug` implementation for SMTP settings
redacts the password, and `config get` / `config list` never expose it.

File permissions:

```bash
chmod 700 ~/.config/herald ~/.local/share/herald
chmod 600 ~/.config/herald/config.toml
```

`init` sets these modes on Unix. `doctor` warns when the config is readable by
group or other, or when the data directory is not `0700`.

## Example configs

### Demo

The default. No network, no model, no account.

```toml
default_provider = "fake"

[policy]
allow_send = false
max_body_chars = 8000
approval_ttl_hours = 72

[model]
backend = "none"
context_size = 4096
threads = 4
```

### Maildir, read-only

Use this to try real mail without giving Herald any ability to send.

```toml
default_provider = "maildir"

[maildir]
path = "~/Mail"
from = "Me <me@example.com>"

[policy]
allow_send = false
```

With `allow_send = false`, a send plan is rejected by policy before an approval
is created. Even with `--enable-send`, a missing `[maildir.smtp]` block means
`send_reply` returns `provider_not_configured`.

### Maildir with SMTP

```toml
default_provider = "maildir"

[policy]
allow_send = true
max_body_chars = 8000
approval_ttl_hours = 72

[maildir]
path = "~/Mail"
sent_path = "~/.Sent"
from = "Me <me@example.com>"

[maildir.smtp]
host = "smtp.example.com"
port = 587
username = "me@example.com"
password_env = "HERALD_SMTP_PASSWORD"
security = "starttls"
```

```bash
export HERALD_SMTP_PASSWORD='...'
herald doctor
```

### Local model, managed sidecar

Herald runs a pinned GGUF through `llama-server` on loopback.

```toml
default_provider = "maildir"

[maildir]
path = "~/Mail"

[model]
backend = "managed"
manifest = "manifest.json"
context_size = 4096
threads = 4
```

```bash
export HERALD_LLAMA_SERVER=/usr/local/bin/llama-server
herald model pin ~/Downloads/Qwen3.5-0.8B-Q4_K_M.gguf --name "Qwen3.5-0.8B" \
  --repo gguf/Qwen3.5-0.8B-GGUF --revision <commit> --quantization Q4_K_M
herald model status
herald ask "summarize the latest from Sam" --engine auto
```

### Local model, existing OpenAI-compatible server

Use this when you already run Ollama, LM Studio, or vLLM on the same machine.

```toml
[model]
backend = "openai"
endpoint = "http://127.0.0.1:11434/v1"
model_name = "qwen3.5:0.8b"
# api_key_env = "HERALD_MODEL_API_KEY"
```

The endpoint can be remote. If it is not loopback, message content leaves the
machine. `doctor` prints the endpoint so the choice is visible. Herald does not
send anything to a Herald-operated service under any configuration.

### X for replies and DMs

```toml
default_provider = "x"

[policy]
allow_send = true

[x]
handle = "@you"
headless = true
close_after = true
```

Run `herald x login` once. Replies and DMs still need `herald approve`;
there is no posting lane (ADR 0011).

## Validation

`herald doctor` is the configuration checker. It reports:

| Check | Failure means |
| --- | --- |
| Config file parses | A syntax error or an unknown key. The path is printed. |
| Data directory is writable, mode 0700 | Permissions or a missing parent. |
| Database opens | A corrupt file or an incompatible schema version. |
| `default_provider` has an adapter | The provider is `gmail` or `x`, which are planned. |
| Send policy | `allow_send = true` is reported as a warning, not an error. |
| Maildir `path` exists | The sync tool is not configured or the path is wrong. |
| Sent folder exists or will be created | Sends would succeed but fail verification. |
| SMTP host and port are set | Wrong host or port. |
| SMTP password resolves | Auth would fail at the first send; the provider becomes read-only. |
| Plaintext password in config | A warning to move it to the environment. |
| IMAP host/port/folder | Shown when `[imap]` is configured; a live login + examine probe runs through the provider health check, so auth and connectivity problems fail the check with the server's reason. |
| Model manifest and file | Download incomplete or manifest mismatch. |
| `llama-server` binary found | The managed backend cannot start. |

Each check prints `ok`, `warn`, or `fail`. A `fail` exits `1`. `--json` returns
`{"status","problems","warnings","checks":[...],"generated_at"}`.

## Related

- [cli.md](cli.md): every command and its output.
- [providers.md](providers.md): Maildir layout, SMTP behaviour, Sent verification.
- [architecture.md](architecture.md): where configuration sits in the crate graph.
- [decisions/0006-inference-backends.md](decisions/0006-inference-backends.md): why managed is the default backend.
---

[Herald README](../README.md) · [Security](../SECURITY.md) · [Contributing](../CONTRIBUTING.md) · [Privacy](../PRIVACY.md) · [Changelog](../CHANGELOG.md)
