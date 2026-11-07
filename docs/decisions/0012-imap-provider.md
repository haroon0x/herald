# 0012: IMAP provider (read-only, Gmail, app passwords)

Date: 2026-11-07
Status: Accepted

## Context

[ADR 0004](0004-email-provider-maildir-smtp.md) shipped Maildir + SMTP and
deliberately left IMAP for later: IMAP needs connection state management and
a server-side search model, and doing it before the approval path was solid
meant debugging two hard things at once. The v1.1 release adds the first
real-provider send-adjacent capability without adding send authority.

Cloud mail (Gmail, Outlook) does not offer free IMAP with the user's normal
password: Gmail requires an app password (generated under 2-step
verification) and refuses basic-password IMAP login. OAuth is the wrong
shape for a local desktop app: it needs a hosted consent flow and a client
secret an app cannot keep. App passwords are worse for revocation and better
for a first release. This ADR records using them deliberately.

On 2026-11-07 the owner scoped the provider: Gmail first, on-demand fetch
with SQLite dedupe, and reads plus flag updates (mark-seen on open).

## Decision

`crates/herald-provider/src/imap.rs` implements the seven-method
`CommunicationProvider` trait over the `imap` crate with TLS via rustls +
webpki-roots. `imap` builds with default features disabled so no native-TLS
C library enters the graph.

- **One short-lived session per operation.** Connect, log in, select, fetch,
  log out — all inside `spawn_blocking` with a 10 s connect timeout and a 30 s
  all-in operation timeout. No polling, no IDLE, no persistent socket, no
  background connection to leak. Health performs the same session in
  read-only (`EXAMINE`) mode.
- **Gmail first.** Host/port/folder are configurable, but the default is
  `imap.gmail.com:993`, folder `INBOX`, TLS on. Gmail-specific behaviour
  relied on is deliberately minimal: app-password LOGIN, FLAGS, and
  `BODY.PEEK[HEADER.FIELDS (...)]` — all standard IMAP4rev1 that Gmail honors.
  No Gmail-specific API (X-GM-* attributes) is read.
- **App passwords only, never stored in the clear.** Same pattern as SMTP:
  `password_env` names an environment variable; a plaintext `password` key
  is parseable but `herald doctor` warns, like SMTP. The registry refuses to
  build the adapter (with a warning) when a username has no password source.
- **Header-first reads.** `list_recent` fetches headers and flags only;
  bodies stay on the server until a thread is opened, mirroring "metadata
  only" retention. Deduplication reuses the existing SQLite
  `(provider, message_id)` key for free.
- **"Threads" are References-rooted groups.** A message's thread id is its
  first normalized `References` id (or its own `Message-ID`). `get_thread`
  re-fetches the headers and groups by that root; a reply whose root is
  outside the 500-header window shows that reply alone. Bodies fetched per
  thread are bounded at 32 messages and 256 KB each.
- **Mark-read on open.** When `mark_read_on_open` (default true) is on and
  any fetched message is unread, the session upgrades from `EXAMINE` to
  `SELECT` and `STORE +FLAGS (\Seen)`. This is the one server mutation the
  provider performs; it mirrors the X adapter's local seen-ledger semantics
  ("opening a thread means seen") but is server-side, so it is config-
  gated and can be turned off (`mark_read_on_open = false` keeps reads
  side-effect free). A failed STORE logs a warning and never fails the
  read. It is idempotent, so no approval queue entry exists for it.
- **No send, ever in v1.1.** `prepare_reply` and `send_reply` are
  `NotConfigured`. Sending over SMTP submission comes later, after its own
  design, tests, and canary.
- **Local search.** `search` matches sender, subject, and message id over
  fetched headers — the same local substring model as X and maildir. IMAP
  `SEARCH` criteria were deliberately not used: Gmail's partial criterion
  support makes it unreliable as a contract (the roadmap allowed "local
  filtering otherwise").
- **Bounds are fail-closed.** Header fetch capped at 500 messages; unread
  filter and search apply locally after the fetch, so a weird mailbox cannot
  turn one command into an unbounded allocation.

## Why these choices

- **App passwords first.** A desktop app cannot do OAuth without a hosted
  consent flow; app passwords are a deliberate, documented service-shape
  choice, not an oversight. OAuth is a later ADR.
- **Short-lived sessions.** A persistent connection would be v1.1's hardest
  failure mode (half-open sockets, keepalive, reconnect storms). A session
  per command trades a few round trips for statelessness; Gmail tolerates the
  call volume because reads are explicit commands.
- **Flags only.** The provider mutates one per-message bit (`\Seen`) in
  response to an explicit read the user initiated. It never writes subject,
  body, or labels, and it never sends. The approval model is unaffected:
  no approval exists for a `STORE`.
- **Threading parity.** References-rooting matches the mental model users
  already get from maildir and from the X adapter's conversation threads,
  without needing Gmail's threading API.

## Trade-offs

- **Gmail support is app-password only.** Accounts without 2-step
  verification cannot use the adapter; the failure mode is a login rejection
  surfaced as `NotAuthenticated`, not a workaround.
- **No push.** New mail arrives at the next command (deliberate: iid idle
  is future work, and the roadmap's exit criteria are on-demand).
- **A reply's parent may be missing** when it falls outside the 500-header
  window — the thread shows the fetched part, honest about what it has.
- **`imap-proto` carries a future-incompat warning** (the crate's internal
  transmute idiom). Acceptable today; revisit if the warning hardens into an
  error in a future Rust.

## Consequences

- New `ProviderId::Imap` (serde `"imap"`); `gmail` remains the planned enum
  variant for a future API provider and is **not** this provider.
- `[imap]` config section with `HERALD_IMAP_*` env overrides; `herald doctor`
  probes it (login + examine, read-only) and warns on plaintext passwords.
- Live verification still needs the owner: a real Gmail app password and one
  real `inbox`/`thread` round trip. Everything automated is covered below.
- Envelope fetch uses `BODY.PEEK[HEADER.FIELDS (...)]` so reads never set
  the server's `\Seen` bit by fetching (the flag is only changed by the
  deliberate `STORE` in `get_thread`).

## When to revisit

- OAuth or XOAUTH2: a new ADR with a consent-flow design; it changes the
  credential story wholesale, not just this file.
- IDLE or background unread: a daemon-shaped change (connection lifecycle,
  reconnect policy, battery/laptop behavior); not a flag.
- IMAP sending: a new provider capability plus the full approval/canary
  treatment. Never by quietly reusing this adapter's read path.
