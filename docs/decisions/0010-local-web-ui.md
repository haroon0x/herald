# 0010: A local browser UI before a desktop shell

Date: 2026-11-05
Status: Accepted

## Context

Herald is a CLI. The owner asked for a GUI, and chose a Tauri desktop app.
Building Tauri needs `webkit2gtk-4.1`, `gtk+-3.0`, and related system
development packages. They are not installed on the target machine, `sudo`
needs a password, and no agent can install them non-interactively.

Meanwhile the shape of the problem is clear: every capability already exists as
a CLI command with a stable `--json` envelope, and every write already stops at
the approval queue or the autopost rails. A GUI is a second *surface*, not a
second execution path.

## Decision

Ship `herald ui`: a browser UI served by the existing binary on loopback, over
the same commands and the same approvals. Treat it as the frontend that a Tauri
shell can wrap unchanged once the system packages exist.

- **Loopback only.** The server binds `127.0.0.1`. It refuses to start on a
  public address.
- **Per-run token.** A random 128-bit token is generated at startup, printed in
  the URL, injected into the page, and required in `X-Herald-Token` on every
  `/api/*` call. Missing or wrong token is a 403 JSON error.
- **Origin check.** A browser-supplied `Origin` must match the bound loopback
  origin, so a cross-site page cannot drive the API even if it guesses the port.
- **No second execution path.** Every API handler shells out to this same
  binary with `--json` and the server's `--data-dir`/`--config`. There is no
  database access, no provider call, and no send path in the UI server. The
  approval semantics are exactly `herald approve`.
- **Untrusted content stays text.** The page renders senders, subjects, bodies,
  and error text with `textContent`, never `innerHTML`. The CSP is
  `default-src 'self'` with no inline script or style.
- **One CLI at a time.** A mutex serialises subprocesses, so two browser tabs
  cannot open two X browsers on the same profile.
- **Bounded input.** Request bodies are capped at 64 KiB; ids, instructions,
  limits, scopes, and autopost actions are validated before they reach the CLI.

Endpoints are deliberately explicit (`/api/inbox`, `/api/thread`,
`/api/approvals`, `/api/approve`, `/api/reject`, `/api/reconcile`,
`/api/autopost`, `/api/ask`, `/api/providers`, `/api/meta`), not a generic
"run any command" proxy.

## Why this shape

- It works today with no new system packages and no Node runtime.
- The safety properties do not fork: there is one approval implementation, one
  body hash check, one idempotency ledger.
- The page is plain HTML/CSS/JS embedded in the binary, so packaging is still a
  single executable. A Tauri shell later gets the same assets and API.
- The token + Origin + CSP combination is appropriate for a local single-user
  tool, without pretending it is a hardened network service.

## Trade-offs

- **Token in the URL.** It can land in browser history. It is loopback-only and
  per-run, and the page sets `Referrer-Policy: no-referrer`.
- **Subprocess per request.** Slower than an in-process call, and X reads can
  take a minute. A global mutex means the first paint waits for the longest
  command.
- **No native window.** It is a browser tab, not a desktop app. Tauri remains
  the plan once the build dependencies are available.
- **Not multi-user.** No accounts, no TLS, no remote access. Binding beyond
  loopback would need all three and a different review.

## Consequences

- `herald ui` is the GUI for now; the terminal remains the source of truth.
- The frontend under `apps/agent/src/ui/` is reused by a future Tauri wrapper.
- A Tauri build can start only after `libwebkit2gtk-4.1-dev`, `libgtk-3-dev`,
  and the other Tauri Linux prerequisites are installed by the owner.
- DMs still cannot be read from the UI, because X renders them in an embedded
  `xchat` app the adapter does not parse (see `docs/providers.md`).

## When to revisit

- The owner installs the Tauri prerequisites: wrap the same page and API.
- Remote access is wanted: that is a different threat model (auth, TLS,
  multi-user) and needs its own ADR, not a flag.
- The UI needs streaming or long-lived state: the subprocess-per-request model
  is the first thing to replace.
