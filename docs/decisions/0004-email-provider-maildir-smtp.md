# 0004: Email provider — Maildir and SMTP for v1

Date: 2026-11-04
Status: Accepted

## Context

Herald needs one real provider to be more than a demo. The candidates were:

1. **Browser automation** against a webmail UI, using a dedicated Chrome
   profile and CDP.
2. **IMAP** for reading and SMTP for sending, with app passwords or OAuth.
3. **A provider HTTP API**, starting with Gmail.
4. **Maildir on disk**, maintained by the user's existing sync tool, plus SMTP
   submission for sending.

The constraint that decides the order is not which provider is most popular. It
is which one can be made reliable without weakening the safety model, in the
time available for v1.

## Decision

v1 ships `maildir` as the real provider: read from a local Maildir (`new/`,
`cur/`), send through SMTP submission, write a copy to the Sent folder, and
verify the copy before reporting success.

The `fake` provider stays as the demo, test, and eval provider.

`gmail` and `x` remain enum variants marked planned. No adapter exists, and the
CLI reports that instead of pretending.

## Why Maildir and SMTP

- **No new authority surface.** The capability trait is list, read, prepare,
  send, find sent. A Maildir adapter needs nothing else. Browser automation
  invites a "click this" or "run this script" capability, which hands prompt
  injection an execution primitive.
- **Deterministic parsing.** A Maildir message is an RFC 5322 file. Threading
  comes from `Message-ID` and `References`. There is no DOM, no layout change,
  no A/B test, and no captcha.
- **No OAuth client.** A local desktop app cannot keep a client secret, and an
  OAuth consent screen implies a hosted redirect. Maildir plus SMTP needs only
  an app password the user already has.
- **Offline reads.** The mail is already on disk. Reading does not depend on a
  network, a session, or a rate limit.
- **Composes with existing tools.** People who want local mail already run
  `mbsync`, `offlineimap`, or `isync`. Herald reads the result instead of
  competing with it.
- **A clean failure model.** SMTP has explicit reply codes. A 5xx is permanent,
  a 4xx is transient, a timeout is `uncertain`. That maps directly onto the
  action state machine.

## Why not browser automation

- It requires a capability the planner must never have: arbitrary page control.
  Keeping selectors inside the provider module helps, but the adapter still
  depends on a UI that changes without notice.
- Login challenges, captchas, and account warnings are stop-and-report
  conditions. That means the provider fails exactly when the user needs it.
- It fights the terms of service of the services it drives.
- It is the hardest thing to test deterministically, and the safety tests are
  the point.

Browser automation is not on the roadmap. If it ever returns, it needs its own
ADR, a dedicated profile, and a capability surface no wider than the seven
methods in the trait.

## Why not IMAP yet

IMAP is the right second provider, not the right first one.

- It needs a connection state machine: connect, authenticate, select, fetch,
  reconnect, and handle server quirks.
- The auth story is unsettled. App passwords work for Gmail and Outlook today
  but are being narrowed. OAuth needs a client identity a local app cannot
  safely hold.
- Server-side search semantics differ across servers, so the adapter needs a
  fallback path.
- Doing IMAP and the approval path at the same time would mean debugging two
  hard systems at once.

The trait is implementation-compatible with an IMAP adapter: `list_recent`,
`get_thread`, and `find_sent` map cleanly. Adding it does not touch
`herald-core`.

## Tradeoffs

- **A sync tool is required.** Herald does not fetch mail itself in v1. The
  user needs `mbsync` or equivalent. This is documented in the quickstart.
- **No server-side search.** Search is a local substring filter over the recent
  window. That is enough for v1 and honest about its limits.
- **Threading depends on headers.** A message with no `Message-ID` or
  `References` starts its own thread. Herald does not guess a parent.
- **SMTP credentials are long-lived.** An app password is worse for revocation
  than OAuth. The mitigation is environment-only storage and a narrow scope.
- **Sent verification is a heuristic.** A Sent copy can lag a successful SMTP
  transaction. Herald treats a missing copy as `uncertain`, not as proof of
  failure.

## Consequences

- The maildir adapter owns every Maildir and SMTP detail. Nothing in
  `herald-core` knows what a Maildir is.
- The approval path is exercised against a real protocol before a second
  provider exists.
- The failure taxonomy is proven on SMTP before it is applied to IMAP.
- v1.1 can add IMAP read-only without changing the action model.

## When to add IMAP

Add IMAP when all of these hold:

1. The Maildir adapter has shipped for a release without a duplicate-send or
   lost-approval bug.
2. The approval and reconciliation tests pass unchanged against a second
   provider.
3. There is a documented app-password path for at least Gmail and Outlook.
4. The injection corpus passes against headers, bodies, and MIME structure over
   IMAP.

Until then, Maildir is the provider, and the roadmap says so.
