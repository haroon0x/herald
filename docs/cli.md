# CLI reference

`herald` is one binary with one subcommand per action. This page is the command
contract for v1.0 and is kept in sync with
[v1-design.md](v1-design.md) and `apps/agent/src/cli.rs`.

```console
$ herald --help
Read local mail, draft with a local model, send only after approval

Usage: herald [OPTIONS] <COMMAND>

Commands:
  init         Create the data directory, database, and a starter config
  doctor       Check storage, configuration, providers, and inference
  ask          Plan and execute a request. Sends stop at an approval
  plan         Dry run: show the validated plan and policy decision
  inbox        List recent messages from a provider
  thread       Print one thread
  approvals    List outbound actions waiting for a decision
  approve      Approve one pending action and send it
  reject       Reject one pending action. Nothing is sent
  reconcile    Resolve an uncertain action by re-reading the provider
  history      Show recorded actions
  audit        Show the append-only audit log
  providers    List providers and their health
  x            X adapter commands
  autopost     Inspect or flip the scoped autopost lane
  config       Read configuration values
  model        Manage the model artifact
  eval         Score the checked-in command set against an engine
  completions  Print shell completions

Options:
      --data-dir <DATA_DIR>  Override the data directory
      --config <CONFIG>      Path to config.toml
      --json                 Emit one JSON object on stdout
      --no-color             Disable ANSI colors. NO_COLOR is also honored
      --log <LOG>            Log filter, e.g. `info` (default: HERALD_LOG or warn)
  -h, --help                 Print help
  -V, --version              Print version
```

Global options apply to every subcommand and can appear before or after it.

## Output contract

Human output uses an aligned label column (`provider:`, `decision:`) and is
colored only on a TTY. Secrets are never printed.

`--json` prints exactly one object on stdout and nothing else. Success:

```json
{"ok":true,"command":"ask","data":{...}}
```

Failure:

```json
{"ok":false,"error":{"code":"unresolved_target","message":"no thread matches \"...\"","hint":null}}
```

Warnings and diagnostics go to stderr, so `herald ... --json | jq` is always
safe. The envelope is stable; the shape of `data` is documented per command.

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | Success. For `ask` with a send, this means an approval was created, not that mail was sent. |
| `1` | Runtime failure: validation, configuration, storage, provider, or model. |
| `2` | Usage error: unknown command, bad flag, missing argument. |
| `3` | Rejected by policy: sending disabled or body too long. |
| `4` | Stale or changed approval: expired, target changed, or body hash mismatch. |
| `5` | `eval` scored below `--min-score`. |

`approve` and `reconcile` return `0` when they resolve an action, including a
resolution of `uncertain` or `failed`. Read `status` in the output, not just the
exit code.

## Error codes

| Code | Cause | Exit |
| --- | --- | --- |
| `policy_rejected` | `allow_send = false`, or the body exceeds `max_body_chars` | 3 |
| `approval_conflict` | Approval expired, target changed, body changed, or the body is gone | 4 |
| `invalid_plan` | Plan failed validation for a reason other than clarification | 1 |
| `invalid_state` | Command does not apply in the action's current state | 1 |
| `unresolved_target` | No provider data matches the target | 1 |
| `ambiguous_target` | More than one thread matches; narrow the target | 1 |
| `unknown_provider` | `--provider` is not a known id | 1 |
| `no_adapter` | Provider is planned and has no adapter in this build | 1 |
| `no_thread` | An uncertain action has no recorded thread to look in | 1 |
| `not_found` | Action, thread, file, or config key does not exist | 1 |
| `not_authenticated` | Provider credentials missing or rejected | 1 |
| `provider_not_configured` | Maildir has no SMTP block, or no `from` address | 1 |
| `provider_timeout` | Provider did not answer before the deadline | 1 |
| `rate_limited` | Provider asked the client to slow down | 1 |
| `provider_layout` | Provider data did not match the expected shape | 1 |
| `unsupported` | Provider cannot perform this operation | 1 |
| `provider` | Any other provider failure | 1 |
| `model` | Inference backend failed to start or return usable output | 1 |
| `model_not_configured` | `--engine model` without a configured backend | 1 |
| `model_missing` | Manifest names a file that is not on disk | 1 |
| `model_not_pinned` | Manifest still has a placeholder digest | 1 |
| `config` | Config missing, unreadable, invalid, or an invalid env override | 1 |
| `storage` | Database cannot be opened or a transition was refused | 1 |
| `io` | Filesystem failure outside storage | 1 |
| `internal` | A bug or an unexpected condition | 1 |

## Commands

### `herald init`

```text
herald init [--force]
```

Creates the data directory (mode 0700 on Unix), opens the database, and writes
the annotated starter config. Never overwrites without `--force`, never writes a
password, never contacts the network.

```console
$ herald init
Herald 1.0.0
data:       /home/you/.local/share/herald
database:   /home/you/.local/share/herald/herald.db
config:     /home/you/.config/herald/config.toml (written)
manifest:   /home/you/.local/share/herald/models/manifest.json (written)
```

`data`: `{"data_dir","database","config","config_written","manifest","manifest_written"}`.

### `herald doctor`

```text
herald doctor
```

Prints one line per check with `ok`, `warn`, or `fail`, then a summary. Exits
`1` if any check is `fail`; `warn` does not fail the command.

```console
$ herald doctor
[ok]   version                1.0.0 (linux)
[ok]   data dir               /home/you/.local/share/herald
[ok]   data perms             700
[ok]   config                 /home/you/.config/herald/config.toml
[ok]   send policy            disabled until an approval is enabled
[ok]   storage                4 messages, 9 audit events, 0 pending approval(s)
[ok]   provider fake          ready
[ok]   provider maildir       ready
[ok]   provider gmail         planned; no adapter in this build
[ok]   provider x             planned; no adapter in this build
[ok]   maildir                /home/you/Mail
[ok]   sent folder            /home/you/Mail/.Sent (exists)
[ok]   smtp                   smtp.example.com:587 (Starttls)
[ok]   smtp auth              password available
[ok]   model                  none; rules only

0 problem(s), 0 warning(s). Herald can run.
```

Checks include: config parse, data permissions, storage counts, stale
`executing` actions, each provider's health, Maildir root, Sent folder, SMTP
password resolution, plaintext-password warning, model manifest, model file,
and the `llama-server` binary.

`data`:

```json
{"status":"ok","problems":0,"warnings":0,
 "checks":[{"name":"version","status":"ok","detail":"1.0.0 (linux)"}],
 "generated_at":"2026-11-04 11:20:00Z"}
```

### `herald ask`

```text
herald ask "<instruction>" [--provider P] [--engine rules|auto|model]
                           [--enable-send] [--thread ID] [--body-env VAR]
```

Plans the instruction, validates it, applies policy, and runs it.

- Reads execute against the provider.
- Drafts are prepared locally and printed; nothing is sent.
- Sends create a pending approval when policy allows. The output shows the
  provider, resolved thread, exact body, SHA-256 hash, action id, and expiry.

`--engine` defaults to `auto`: rules first, model fallback if one is configured.
`--enable-send` enables the approval path for this command. It does not send.
`--thread <id>` pins the target to an exact thread id; use it when a phrase is
ambiguous or when scripting. The id is still re-resolved and checked at approve
time.
`--body-env <VAR>` reads the exact outbound body from the environment variable
`VAR` instead of any model draft (no drafting is attempted). It is for callers
that already hold the body — the `herald ui` composer is the caller this exists
for — and the body never travels in argv. An unset or whitespace-only variable
is rejected with `invalid_plan`. The plan still passes validation and policy:
with `--enable-send` the body is queued as an approval, never sent directly.

A read:

```console
$ herald ask "check what is unread"
provider:   fake
operation:  list_unread_messages
target:     check what is unread

* Alex Founder <alex@example.com>   Interview next steps   [t-founder]
  id: m-founder-2
* Alex Founder <alex@example.com>   Interview next steps   [t-founder]
  id: m-founder-1
new:        2 recorded
```

A post requested by topic is drafted by the configured model and then follows
the normal approval path:

```console
$ herald ask 'tweet about the local-first release' --provider x --enable-send
provider:   x
operation:  create_post
target:     your timeline
body:       Shipped the local-first release. Your comms, your machine.
hash:       0754...
```

An original post goes to the timeline, not a conversation:

```console
$ herald ask 'Post exactly: "Shipping the X adapter today."' --provider x --enable-send
provider:   x
operation:  create_post
target:     your timeline
body:       Shipping the X adapter today.
hash:       8c1d2f...

Approval required
action:     2b7e9f04-...
provider:   x
target:     your timeline
thread:     timeline
body:       Shipping the X adapter today.
hash:       8c1d2f...
```

With the autopost lane fully enabled and every rail passing, the same command
publishes and verifies instead of queuing (see `herald autopost status`).
Otherwise it queues exactly like a reply. `timeline` identifies no recipient:
a post never resolves to a person.

A send that names no conversation fails closed:

```console
$ herald ask 'Send exactly: "Thanks, I will review this."' --enable-send
error: no thread matches "Send exactly: \"Thanks, I will review this.\""
```

Name a recipient or pass `--thread` to make it resolvable:

```console
$ herald ask 'Send exactly: "Thanks, I will review this." to the founder' --enable-send
$ herald ask 'Send exactly: "Thanks, I will review this."' --thread t-founder --enable-send
```

A send that resolves:

```console
$ herald ask 'Reply to the latest from Alex Founder, send exactly: "Thanks, I will review this."' --enable-send
provider:   fake
operation:  send_reply
target:     latest message from Alex Founder
body:       Thanks, I will review this.
hash:       4f9c2b7e0d1a8c3f5e6b9d0a2c4e8f17b3d5a9c1e7f2b4d6a8c0e3f5b7d9a1c2

Approval required
action:     7f3c1a2e-9b04-4c6d-8f1a-2d5e6b7c8d90
provider:   fake
target:     latest message from Alex Founder
thread:     t-founder
body:       Thanks, I will review this.
hash:       4f9c2b7e0d1a8c3f5e6b9d0a2c4e8f17b3d5a9c1e7f2b4d6a8c0e3f5b7d9a1c2
expires:    2026-11-07 11:20:00Z

Nothing has been sent. To send exactly this body:
  herald approve 7f3c1a2e-9b04-4c6d-8f1a-2d5e6b7c8d90
To discard it:
  herald reject 7f3c1a2e-9b04-4c6d-8f1a-2d5e6b7c8d90
```

`data` for a send:

```json
{"status":"awaiting_approval","action_id":"7f3c...","provider":"fake",
 "operation":"send_reply","target":"latest message from Alex Founder",
 "thread_id":"t-founder","body":"Thanks, I will review this.",
 "body_hash":"4f9c...","expires_at_ms":1793877600000}
```

Other `status` values: `succeeded` (with `outcome`), `clarification_required`
(with `question`), `no_plan`.

### `herald plan`

```text
herald plan "<instruction>" [--provider P] [--engine rules|auto|model]
                            [--enable-send] [--thread ID]
```

Dry run. Prints the plan, the validation result, and the policy decision. It
does not record an action, create an approval, or call a provider.

```console
$ herald plan 'Send exactly: "Thanks, I will review this."'
provider:   fake
operation:  send_reply
target:     Send exactly: "Thanks, I will review this."
body:       Thanks, I will review this.
hash:       4f9c2b7e0d1a8c3f5e6b9d0a2c4e8f17b3d5a9c1e7f2b4d6a8c0e3f5b7d9a1c2
decision:   rejected
detail:     sending is disabled by policy

Dry run: no action was recorded and no provider was changed.
```

`decision` is `allowed`, `needs_approval`, or `rejected`. `data`:
`{"status","provider","operation","target","decision","detail","would_send"}`.

### `herald inbox`

```text
herald inbox [--scope all|feed|mentions|dms|notifications] [--unread] [--limit N] [--provider P]
```

Lists recent messages. Default limit 10 and default scope `all` (for X:
mentions plus DMs). `--scope feed` reads the home timeline, `--scope mentions`
only public mentions, `--scope dms` only conversations. Providers with one
surface ignore the scope. Unread messages are marked with `*`.

```console
$ herald inbox --unread
* Alex Founder <alex@example.com>   Interview next steps   [t-founder]
  id: m-founder-2
```

The X home timeline:

```console
$ herald inbox --provider x --scope feed --limit 5
* @ada                            Post by @ada             [mention:901]
  id: mention:901
```

`data`: `{"provider","unread_only","messages":[{"id","thread_id","sender","subject","timestamp_ms","unread"}],"count"}`.

### `herald thread`

```text
herald thread <id> [--provider P]
```

```console
$ herald thread t-founder
subject:    Interview next steps
messages:   2

--- Alex Founder <alex@example.com>  2026-11-04 10:20:00Z ---
Great speaking with you. Are you still interested? Let me know.

--- Alex Founder <alex@example.com>  2026-11-04 11:20:00Z ---
Following up on the above.
```

Display truncates each body at 2,000 characters. `data`:
`{"id","provider","subject","messages":[{"id","thread_id","sender","subject","body","timestamp_ms","unread"}]}`.

### `herald approvals`

```text
herald approvals
```

Lists pending approvals. Expired approvals are cancelled at startup, so they
do not appear here. The body is shown as a one-line preview; `ask` prints the
full body when it creates the approval.

```console
$ herald approvals
1 approval(s) waiting

action:     7f3c1a2e-9b04-4c6d-8f1a-2d5e6b7c8d90
provider:   fake
target:     latest message from Alex Founder
thread:     t-founder
body:       Thanks, I will review this.
hash:       4f9c2b7e0d1a8c3f5e6b9d0a2c4e8f17b3d5a9c1e7f2b4d6a8c0e3f5b7d9a1c2
expires:    2026-11-07 11:20:00Z
```

`data`: `{"approvals":[{"action_id","provider","target","thread_id","body","body_hash","created_at_ms","expires_at_ms"}],"count"}`.

### `herald approve`

```text
herald approve <action-id>
```

The only command that can send. It performs, in order:

1. Load the action. Refuse unless it is `awaiting_approval` (`invalid_state`).
2. Load the approval body. Refuse if it is gone (`approval_conflict`).
3. Refuse if `expires_at_ms` has passed, move to `cancelled`, delete the body.
4. Re-resolve the target. Refuse if the thread changed (`approval_conflict`).
5. Recompute the body hash. Refuse on mismatch (`approval_conflict`).
6. Prepare, send with the client token, verify, and record the receipt.

```console
$ herald approve 7f3c1a2e-9b04-4c6d-8f1a-2d5e6b7c8d90
target:     latest message from Alex Founder
thread:     t-founder
body:       Thanks, I will review this.
hash:       4f9c2b7e0d1a8c3f5e6b9d0a2c4e8f17b3d5a9c1e7f2b4d6a8c0e3f5b7d9a1c2
action:     7f3c1a2e-9b04-4c6d-8f1a-2d5e6b7c8d90
status:     sent
detail:     sent and verified as m-sent-7f3c1a2e-9b04-4c6d-8f1a-2d5e6b7c8d90
message:    m-sent-7f3c1a2e-9b04-4c6d-8f1a-2d5e6b7c8d90
```

A timeout is `uncertain`, not a failure:

```console
$ herald approve 7f3c1a2e-9b04-4c6d-8f1a-2d5e6b7c8d90
action:     7f3c1a2e-9b04-4c6d-8f1a-2d5e6b7c8d90
status:     uncertain
detail:     delivery timed out; run `herald reconcile` to resolve it

Resolve it with: herald reconcile 7f3c1a2e-9b04-4c6d-8f1a-2d5e6b7c8d90
```

`data`: `{"action_id","status","verified","message_id","detail"}`. `status` is
`sent`, `uncertain`, or (from reconcile) `failed`.

### `herald reject`

```text
herald reject <action-id>
```

Cancels an `awaiting_approval` action, moves it to `cancelled`, and deletes the
retained body. Nothing is sent.

`data`: `{"action_id","status":"cancelled","sent":false}`.

### `herald reconcile`

```text
herald reconcile <action-id>
```

Resolves an `uncertain` action by asking the provider whether the message
exists (`find_sent`). It never sends.

- Message found and verified: state `succeeded`.
- Message absent: state `failed`.
- Provider unreachable: the action stays `uncertain` and the command fails.

A stale `executing` action older than 60 seconds is first moved to `uncertain`,
then reconciled.

```console
$ herald reconcile 7f3c1a2e-9b04-4c6d-8f1a-2d5e6b7c8d90
action:     7f3c1a2e-9b04-4c6d-8f1a-2d5e6b7c8d90
status:     sent
detail:     the provider confirmed the message; marked succeeded
message:    m-sent-7f3c1a2e-9b04-4c6d-8f1a-2d5e6b7c8d90
```

`data`: `{"action_id","status","verified","message_id","detail"}`.

### `herald history`

```text
herald history [--limit N]
```

Newest first. Default limit 20.

```console
$ herald history --limit 2
ACTION                               OPERATION                STATE        PROVIDER           TARGET
7f3c1a2e-9b04-4c6d-8f1a-2d5e6b7c8d90 send_reply               succeeded    fake               latest message from Alex Founder
c1d2e3f4-5a6b-4c7d-8e9f-0a1b2c3d4e5f list_unread_messages     succeeded    fake               unread
```

`data`: `{"actions":[{"action_id","provider","operation","target","thread_id","state","created_at_ms","completed_at_ms","content_hash"}],"count"}`.

### `herald audit`

```text
herald audit [--limit N]
```

Newest first. Default limit 30. Events never contain bodies or secrets.

```console
$ herald audit --limit 3
2026-11-04 11:20:00Z  succeeded            7f3c1a2e-...  sent message m-sent-... to thread t-founder
2026-11-04 11:20:00Z  executing            7f3c1a2e-...  sending to thread t-founder
2026-11-04 11:20:00Z  awaiting_approval    7f3c1a2e-...  send requires explicit confirmation
```

`data`: `{"events":[{"action_id","at_ms","status","detail"}],"count"}`.

### `herald providers`

```text
herald providers
```

```console
$ herald providers
PROVIDER   STATUS             DEFAULT  DETAIL
fake       ready                       adapter available
maildir    ready              *          adapter available
gmail      planned                      no adapter in this build; plans are inert
x          not configured               adapter exists but is not configured; run `herald init`
```

`health()` is a real check: for `x` it launches the browser and reads the home
timeline, so it reports `not authenticated` until `herald x login` succeeds.

`data`: `{"providers":[{"provider","status","detail","implemented","default"}]}`.

### `herald x login`

```text
herald x login
```

Opens a **visible** Chrome window on the dedicated X profile, navigates to the
login page, and waits (up to 10 minutes) for a signed-in home timeline. The user
signs in. Herald does not read cookies, type credentials, or complete a
challenge; if a captcha or account warning appears, the command reports it and
stops.

Requires an `[x]` section; without one it exits with `not_configured` and does
not launch a browser.

```console
$ herald x login
Opening a visible Chrome window on the dedicated X profile.
profile:    /home/you/.local/share/herald/profiles/x
Sign in yourself. Herald does not read cookies or type credentials.
signed in: the X adapter can read this profile.
```

`data`: `{"status":"authenticated","provider":"x","profile":"..."}`.
Exit code 8 (`timeout`) if no signed-in timeline appears.

### `herald autopost`

```text
herald autopost status
herald autopost off
herald autopost on
```

`status` prints every rail and the effective send path. `off` writes the kill
switch file `<data-dir>/autopost.disabled`; `on` removes it. Neither edits
`config.toml`: configuration must also set `allow_autopost = true` and
`allow_send = true`.

```console
$ herald autopost status
Autopost
configured:   off
kill switch:  off
quiet hours:  22:00-06:00 UTC (inactive)
daily budget: 0/5 used today (UTC)
remaining:    5 post(s)
quality floor: 7/10
send path:    approval only

Everything still goes through the approval queue. Nothing autoposts.
```

`data`: `{"status","configured","kill_switch","quiet_hours","quiet_now",
"daily_limit","used_today","remaining_today","min_quality","today_start_ms"}`.
`off`/`on` emit `{"status","kill_switch","configured","path"}`.

### `herald ui`

```text
herald ui [--port PORT] [--open]
```

Serves the local browser UI on `127.0.0.1` and prints a URL with a per-run
token. `--port 0` picks a free port. `--open` launches the printed URL with
`xdg-open` (failures are ignored, the server keeps running; Unix only). On
start, the server writes the URL to `<data-dir>/ui-url` with mode `0600`
(replaced atomically), which is how a supervised `herald ui` hands the URL to
you — see [contrib/README.md](../contrib/README.md) for a systemd user unit.
Every API call needs the token; the page cannot be opened without it. See
[ui.md](ui.md).

```console
$ herald ui --open
Herald UI  http://127.0.0.1:7878/?token=4f1c…
Loopback only. Every API call needs the token. Ctrl+C stops it.
Approvals and autopost are the same rails as the CLI.
```

This command does not return while the server runs; exit code 1 if the port
cannot be bound.

### `herald config`

```text
herald config get <key>
herald config list
herald config path
herald config init [--force]
```

`path` and `init` work even when the existing config is invalid. `get` prints
the resolved value only; `list` prints `key = value` pairs; secrets are never
included.

```console
$ herald config get policy.allow_send
false

$ herald config list
default_provider = fake
maildir.path = /home/you/Mail
model.backend = none
policy.allow_send = false
policy.approval_ttl_hours = 72
policy.max_body_chars = 8000
```

`data`: `{"key","value"}` for `get`; `{"config":{...}}` for `list`;
`{"path","exists"}` for `path`; `{"path","written"}` for `init`.

### `herald model`

```text
herald model download
herald model verify
herald model status
herald model pin <file> [--name N] [--repo R] [--revision REV]
                        [--quantization Q] [--manifest PATH]
```

`download` fetches the pinned artifact with a resumable HTTP request and
verifies the SHA-256. It refuses a placeholder digest (`model_not_pinned`) and a
mismatch. `verify` re-checks an existing file. `status` reports the backend and
artifact state. `pin` hashes a local GGUF and writes a manifest with provenance;
it never downloads anything.

```console
$ herald model status
backend:    managed
manifest:   /home/you/.local/share/herald/models/manifest.json
name:       Qwen3.5-0.8B
quantization: Q4_K_M
revision:   13ac0662
file:       /home/you/.local/share/herald/models/Qwen3.5-0.8B-Q4_K_M.gguf (504238208 bytes)
sha256:     1f0c2b7e0d1a8c3f…
```

`data` for `status`: `{"backend","endpoint","model_name","manifest"}`. For
`download`: `{"file","verified"}`. For `verify`: `{"file","sha256","verified"}`.
For `pin`: `{"file","sha256","size_bytes","manifest"}`.

### `herald eval`

```text
herald eval [--engine rules|auto|model] [--min-score N]
            [--eval-file PATH] [--provider P]
```

Scores the checked-in command set against an engine. It never executes a read
or a send. `--min-score` is a percentage (0-100, default 0); below it, the
command exits `5`.

```console
$ herald eval --engine rules --min-score 30
PASS [rules] check-mail
PASS [rules] destructive-delete
...
42/42 passed [rules], 100%; rules hits: 42
```

`data`: `{"engine","passes","total","percentage","min_score","below_threshold","rules_hits","results":[{"id","ok","source"|"reason"}]}`.

### `herald completions`

```text
herald completions <bash|zsh|fish|powershell|elvish>
```

Prints a completion script on stdout. It never loads the config or the
database.

```bash
herald completions zsh > ~/.zfunc/_herald
```

## Approval workflow

The full loop, with JSON output and `jq`.

```bash
# 1. Ask for a send. This creates an approval and exits 0.
herald ask 'Reply to the latest from Sam, send exactly: "Thursday works."' \
  --enable-send --json > /tmp/ask.json

jq -r '.data.action_id' /tmp/ask.json

# 2. Inspect the queue. The body is shown because you are the approver.
herald approvals --json \
  | jq -r '.data.approvals[] | [.action_id, .thread_id, .body] | @tsv'

# 3. Approve exactly one id. Never loop over every approval.
id=$(jq -r '.data.action_id' /tmp/ask.json)
herald approve "$id" --json | jq -r '.data.status'

# 4. If it timed out, reconcile instead of re-sending.
herald reconcile "$id" --json | jq -r '.data.status'
```

A refusal is a normal outcome:

```bash
if ! herald approve "$id" --json > /tmp/approve.json; then
  jq -r '.error.code' /tmp/approve.json   # approval_conflict, invalid_state, ...
fi
```

## Reconciliation

A send that times out is `uncertain`. Do not run `approve` again; it refuses
because the action is no longer `awaiting_approval`.

```bash
herald history --json \
  | jq -r '.data.actions[] | select(.state == "uncertain") | .action_id' \
  | while read -r id; do
      herald reconcile "$id" --json | jq -r '"\(.data.action_id) \(.data.status)"'
    done
```

If reconcile fails, the provider is unreachable. The action stays `uncertain`
until a later run; it is never retried blind.

## Scripting

```bash
# Fail if anything is waiting for approval.
test "$(herald approvals --json | jq '.data.count')" -eq 0

# Unread count.
herald inbox --unread --json | jq '.data.count'

# Print senders of unread mail.
herald inbox --unread --json | jq -r '.data.messages[].sender'

# Every failed action in the last page of history.
herald history --json | jq -r '.data.actions[] | select(.state == "failed") | .action_id'

# Dry-run a batch of instructions and keep the policy decisions.
while IFS= read -r line; do
  herald plan "$line" --json | jq -c '{instruction: $line, decision: .data.decision}'
done < instructions.txt
```

## Notes

- `ask` and `plan` are the only commands that accept an instruction. Everything
  else operates on ids and local state.
- Reads use bounded limits (20 for lists, 50 for target resolution). There is
  no "dump the mailbox" command.
- `--json` output is one object, one line, no trailing text. Diagnostics go to
  stderr.
- Human output is stable enough to read but not to parse. Use `--json` for
  scripts.
---

[Herald README](../README.md) · [Security](../SECURITY.md) · [Contributing](../CONTRIBUTING.md) · [Privacy](../PRIVACY.md) · [Changelog](../CHANGELOG.md)
