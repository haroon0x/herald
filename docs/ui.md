# The local UI

`herald ui` serves a browser interface from the same binary, on loopback, over
the same commands and approvals the terminal uses. It is the GUI today; a Tauri
desktop shell can wrap the same page and API later
([ADR 0010](decisions/0010-local-web-ui.md)).

![Herald dispatch desk](../assets/ui-dispatch.png)

## Start it

```bash
herald ui
# Herald UI  http://127.0.0.1:7878/?token=…
```

Open the printed URL. The token is per run and required by every API call; a
URL without it is refused. `--port 0` picks a free port. `Ctrl+C` stops it.

## What it does

| Area | Behaviour |
| --- | --- |
| Scopes | All, Feed, Mentions, Notifications, DMs. Same `herald inbox --scope` reads. |
| Provider | `x`, `fake`, or `maildir`; the default follows the config, preferring `x` when `[x]` is configured. |
| Threads | Clicking an item runs `herald thread`; message bodies are untrusted text. |
| Queue | Pending approvals with the exact body, hash, and expiry. Review opens a confirmation dialog; approve runs `herald approve`, reject runs `herald reject`. |
| Ask | Runs `herald ask`; “Allow this request to create an approval” maps to `--enable-send`. |
| Autopost | Shows the rails and writes the kill switch (`herald autopost off`). |
| Providers | Health table from `herald providers` (X launches a browser; expect a delay). |

Deep links: `/?token=…&provider=x&scope=feed&thread=mention:123` and
`&tab=queue`.

## Security properties

- Binds `127.0.0.1` only. It cannot be reached from another machine.
- Per-run 128-bit token in the URL, injected into the page, required in
  `X-Herald-Token` for every API call. Wrong or missing token: 403.
- `Origin`, when present, must match the loopback origin.
- Content Security Policy `default-src 'self'`; no inline script, no external
  assets, no telemetry.
- The server holds no domain logic. Every action is a subprocess of the same
  CLI, so the approval queue, body hash, autopost rails, and idempotency ledger
  are identical to the terminal.
- Rendered message content uses `textContent`, never `innerHTML`.
- Request bodies are capped at 64 KiB and inputs are validated.

## Limits

- Not a network service: no accounts, no TLS, no remote access.
- One command at a time, so an X read blocks the first paint.
- DMs show empty: X renders them in an embedded `xchat` app the adapter does
  not parse yet.
- `herald x login` stays in the terminal: it needs a visible browser window and
  a human.
