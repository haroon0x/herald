# FAQ

## Why doesn't Herald just send the message?

Because sending is the one action that cannot be undone. A classifier that is
right 90% of the time is good enough to draft; it is not good enough to speak
as you. Herald keeps the final step with the person who owns the account: it
shows the provider, the resolved thread, the exact body, and the hash, and it
waits.

Auto-send is also the feature that turns a prompt injection into a sent
message. If a hostile email can steer the planner, the approval step is what
stops the damage. There is no auto-send setting in v1, and the policy type has
no way to express one.

## Why a local model instead of a cloud API?

Three reasons:

1. **Message content stays on the machine.** Your mail is the most sensitive
   data on your computer. A cloud planner means uploading it.
2. **The task is small.** Herald asks the model to pick one of seven operations
   and, separately, to write a short draft. A 0.8B model does that well when
   the output is grammar-constrained.
3. **It works without a network and without a per-token bill.**

If you already run a model server, you can point Herald at it. That is an
explicit choice, and `doctor` tells you when the endpoint is not local.

## Does it read my whole inbox?

No. Reads are commands with limits: 20 messages for a list, 50 for target
resolution, one thread for a summary. Herald does not scan the mailbox in the
background; there is no scheduler and no daemon.

It also does not store what it reads. Message metadata (sender, subject, time,
unread flag, ids) goes into SQLite for dedupe. Bodies are used to answer the
command and are not persisted. The exception is a pending approval, which keeps
the exact outbound body until you approve, reject, or it expires.

## What leaves the machine?

By default: nothing. There is no telemetry, no update check, and no
Herald-operated server.

Three things can leave, and each requires configuration:

| What | When | Where |
| --- | --- | --- |
| A reply you approved | You run `herald approve` | Your SMTP server |
| A model prompt | `[model] backend = "openai"` with a non-local `endpoint` | That endpoint |
| A model download | You run `herald model download` | The pinned host, over HTTPS |

The Maildir read path is local file I/O. The `fake` provider never touches the
network.

## Can it work offline?

Yes, for everything except sending.

- `fake` provider: fully offline.
- Maildir reads: fully offline, the mail is already on disk.
- Managed model: fully offline once the GGUF is downloaded.
- SMTP send: needs a network path to your submission server.
- OpenAI-compatible backend: needs whatever that endpoint needs.

## Which mail providers work?

v1 ships two: `fake` (seeded demo inbox) and `maildir` (local Maildir plus
SMTP). `gmail` and `x` are enum variants marked planned. They have no adapter,
and the CLI says so instead of pretending.

```console
$ herald providers
PROVIDER   STATUS             DEFAULT  DETAIL
fake       ready                       adapter available
maildir    ready              *          adapter available
gmail      planned                      no adapter in this build; plans are inert
x          planned                      no adapter in this build; plans are inert
```

## Why Maildir and not IMAP yet?

Maildir is a directory of files. It needs no OAuth client, no app review, no
browser, and no terms-of-service negotiation. It also composes with tools
people already run: `mbsync`, `offlineimap`, `isync`. For a first real provider,
that is the shortest path to something reliable.

IMAP is the next step, read-only first, using app passwords. It needs a
connection state machine, a different auth story, and server-side search, and
doing that before the approval path was solid would have meant debugging two
hard things at once. The reasoning is in
[decisions/0004-email-provider-maildir-smtp.md](decisions/0004-email-provider-maildir-smtp.md).

## What happens on a timeout?

For a read: the action fails and you can run the command again. Reads are safe
to repeat.

For a send: the action becomes `uncertain`, not `failed`. The message may have
reached the server. Herald does not retry. You run:

```bash
herald reconcile <action-id>
```

Herald looks for the message in the Sent folder by client token. If it is
there, the action becomes `succeeded`. If it is not, it becomes `failed`, and
you can plan a new send. The full sequence is in
[cli.md](cli.md#reconciliation).

## How do I delete my data?

Everything Herald keeps is under the data directory that `herald doctor` prints.

```bash
herald doctor | grep -E 'data dir|storage'
rm -rf ~/.local/share/herald     # database, models, nothing else
rm -f ~/.config/herald/config.toml
```

Deleting the data directory removes message metadata, action history, the audit
log, and downloaded models. It does not touch your Maildir or your mail server.
Approval bodies are deleted as soon as an approval is approved, rejected, or
expires, so they are not waiting for cleanup.

## Is there a GUI?

No. v1 is a CLI. A TUI and a desktop surface are on the roadmap after the
provider and approval work is done. The CLI has a stable `--json` output
specifically so a GUI can be built on top without changing the core.

## Can I use Ollama or LM Studio?

Yes. Both expose an OpenAI-compatible endpoint.

```toml
[model]
backend = "openai"
endpoint = "http://127.0.0.1:11434/v1"
model_name = "qwen3.5:0.8b"
```

```bash
herald doctor
herald ask "summarize the latest from Sam" --engine auto
```

Use a loopback URL unless you intend to send message content to that machine.
Herald cannot tell a local Ollama from a remote one; it prints the endpoint so
you can.

## How do I add a provider?

Implement `CommunicationProvider`, satisfy the capability checklist in
[providers.md](providers.md), and add the safety tests: injection on every
input surface, idempotent send, honest verification, and no blind retry.

Two rules matter more than the code:

1. Read-only first. Get listing and threading right before you can send.
2. No generic capability. If the trait grows a "run this selector" or "open
   this URL" method, prompt injection becomes remote control.

## Why AGPL-3.0?

Herald handles personal communication and asks for your mail credentials. It
has to be inspectable, and improvements have to stay inspectable, including
when someone offers it as a hosted service. AGPL-3.0-or-later does that. The
reasoning and what is still open (trademark search, a possible Pro tier) are in
[decisions/0002-license-and-product.md](decisions/0002-license-and-product.md).

## Is it safe to run on a work laptop?

It is a local process with the same permissions as your user account. It reads
the Maildir you point it at, writes to its data directory, and connects to the
SMTP host you configure. It does not install a service, open a listening port,
or run in the background. The managed model binds to `127.0.0.1` only.

Check your employer's policy before pointing it at a work mailbox. Herald
cannot make that decision for you.

## Can I run several accounts?

Not in v1. One config, one `default_provider`, one data directory. Running two
accounts means two data directories:

```bash
HERALD_DATA_DIR=~/.local/share/herald-work \
HERALD_CONFIG=~/.config/herald/work.toml \
herald inbox
```

That works because the data directory and config path are both overridable, but
it is not a first-class multi-account feature.

## How big is the model?

Qwen3.5-0.8B Q4_K_M is about 0.5 GB on disk. At the default 4096-token context
it uses a few hundred megabytes of RAM. The 2B variant is roughly 1.5 GB. Both
`model.context_size` and `model.threads` are configurable. The model is
optional: with `backend = "none"`, rules handle the common commands and no
model is loaded.

## How do I try the uncertain / reconcile path?

The demo provider has a deterministic failure hook for this, used by the
integration tests:

```bash
herald ask 'Send exactly: "Noted." to the founder' --enable-send --json > /tmp/ask.json
id=$(jq -r '.data.action_id' /tmp/ask.json)

HERALD_DEMO_FAIL_NEXT_SEND=after herald approve "$id"   # delivered, response lost
herald history                                          # uncertain
herald reconcile "$id"                                  # found in Sent, now succeeded
```

`before` fails before delivery; `after` fails after delivery. In both cases the
action becomes `uncertain`, and `reconcile` decides from provider state instead
of retrying the send. This hook only applies to the `fake` provider.

## X sign-in says "This browser or app may not be secure". Why?

That wording is Google's secure-browser check, not an X error. It fires when
X's Google sign-in is attempted from a browser driven over CDP (Playwright,
Puppeteer, `chromiumoxide`, and Herald before this change). Herald now opens a
plain Chrome window with no automation flags for `herald x login`; sign in
there, close the window, and Herald verifies the session and reuses the
profile. Signing in with an X username/password instead of Google also avoids
that check. Do not use stealth plugins to bypass it: they are an arms race,
violate the provider's terms, and run untrusted code near your session.

## Does Herald use a browser? Can I use Playwright?

Only for X. Mail is still a Maildir on disk plus SMTP. The X adapter drives a
dedicated Chrome profile through `chromiumoxide` because X has no free API for
this use case; it signs in once in a visible window, owns every selector, and
stops on a challenge or unexpected page. That exception is recorded in
[ADR 0008](decisions/0008-x-browser-adapter.md); the general reasoning against
browser automation is in [ADR 0007](decisions/0007-browser-automation.md).

Playwright itself is still not a dependency: it adds a Node runtime and a
driver. `chromiumoxide` is the closest async Chrome DevTools Protocol client in
Rust; `fantoccini` and `thirtyfour` are WebDriver clients; `headless_chrome` is
the synchronous CDP option.

If you want a Playwright-style API in Rust for a different project,
`chromiumoxide` is the closest async Chrome DevTools Protocol client;
`fantoccini` and `thirtyfour` are WebDriver clients; `headless_chrome` is the
synchronous CDP option. For Herald itself, the next provider is IMAP read +
SMTP send, not a browser.

## What is the owl?

Herald's mascot is a messenger owl. It watches the inbox, carries the envelope,
and only leaves after you approve the exact message. The mark and mascot are in
[`assets/`](../assets/README.md) and carry the same license as the project.
## What does Herald not do?

It does not archive, delete, forward, post, follow, or bulk-edit anything. It
does not read attachments. It does not run in the background, notify you, or
sync between machines. It does not auto-send. Each of those is a deliberate
decision recorded in [architecture.md](architecture.md#extension-points) and
[roadmap.md](roadmap.md).
---

[Herald README](../README.md) · [Security](../SECURITY.md) · [Contributing](../CONTRIBUTING.md) · [Privacy](../PRIVACY.md) · [Changelog](../CHANGELOG.md)
