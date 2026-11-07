# 0008: The X adapter is a constrained browser client

Date: 2026-11-04
Status: Accepted
Amended: [0011](0011-no-posting-lane.md) removed original-post creation; the
adapter is now a communications channel only (mentions, DMs, feed,
notifications, approval-gated replies and DMs).

## Context

Herald's owner uses X daily but wants to stop opening x.com: read mentions and
DMs, reply, and send DMs. Original-post creation was initially included here
and later removed by [ADR 0011](0011-no-posting-lane.md). [ADR 0007](0007-browser-automation.md)
recorded the conditions under which browser automation would be acceptable and
recommended against shipping it for mail. Mail still uses Maildir and SMTP.

X has no free, stable read/write API for this use case. The paid API was
considered and priced; the owner rejected it as a per-use cost for what is a
single-user client. Third-party scraping libraries (`browser-use`, Playwright
wrappers, hosted scrapers) were rejected for the reasons in ADR 0007: they add a
runtime, and — for LLM-driven browser agents — they hand the model arbitrary
page control.

That leaves the browser and a human session. The question is not "can Rust
drive Chrome" but "can a browser adapter keep the same safety properties as a
protocol adapter".

## Decision

`crates/herald-provider/src/x.rs` implements the existing seven-method
`CommunicationProvider` trait over `chromiumoxide` 0.9 (Chrome DevTools
Protocol) against a **dedicated profile** at `<data-dir>/profiles/x`.

1. **A human signs in once, in a plain browser.** `herald x login` opens the
   dedicated profile in ordinary Chrome — no CDP, no `--enable-automation`, no
   remote-debugging port — and waits for the window to close. This is not
   politeness: Google's OAuth flow refuses a CDP-controlled browser with
   "This browser or app may not be secure", and bypassing that with stealth
   patches is evasion. Herald never reads cookies, never extracts credentials,
   never types a password, and never completes a challenge. It also never
   points at an everyday Chrome profile.
2. **The capability surface is unchanged.** Only the seven trait methods
   exist. Selectors and URLs are module constants; the planner cannot name a
   selector, a URL, a script, or a "click anything" operation. Thread ids are
   validated (`mention:<digits>`, `dm:<safe id>`) before any
   navigation, so page content can never steer one. The former `timeline`
   thread id is rejected (ADR 0011).
3. **Page text is untrusted.** Parsed tweets and DMs become
   `Untrusted<String>` bodies. Injection fixtures prove that instructions in a
   message do not change a selector, a URL, or a capability.
4. **Hard stops are errors.** A login form means `NotAuthenticated`. A captcha,
   an identity challenge, or an account warning is an `UnexpectedLayout` stop;
   a rate-limit page is `RateLimited`. The adapter does not retry around any of
   them.
5. **Writes are idempotent and verified.** A sent-ledger
   (`<data-dir>/x-sent.json`) maps the client token to its receipt; a replay
   returns the stored receipt. Verification re-reads the surface and requires
   an exact body match from the user's own handle. When that cannot be proven
   the receipt says `verified = false` and the action stays `uncertain`.
   Because attribution needs an identity, writes require `x.handle` and refuse
   to prepare without it.
6. **Unread is local.** X exposes no reliable server-side unread state in the
   DOM, so unread comes from a bounded seen-ledger
   (`<data-dir>/x-seen.json`), not from scraping a badge.
7. **Threads are two namespaces.** `mention:<tweet_id>` is a public
   conversation and `dm:<conversation_id>` is a private conversation. There is
   no timeline/compose thread (ADR 0011).

Post metrics are **not** part of this milestone. The seven-method surface has
no metrics operation, and adding one is a capability change that needs its own
ADR. The adapter can read a post's text and status id; it does not collect
impressions, likes, or reach.

## Why this shape

- It reuses the approval queue, the state machine, the client-token idempotency
  rule, and the `find_sent` reconciliation path without a second execution
  model.
- The `XClient` test seam is scripted: the parsing, idempotency, hard-stop, and
  injection tests run without a browser. Only the CDP wrapper itself is
  untested by unit tests.
- A dedicated profile is the only place a session exists, which keeps the
  blast radius to the X account the user chose to sign in.

## Trade-offs

- **DOM churn.** X changes markup without notice. Selectors are constants with
  tests, but a change breaks the adapter until it is updated. The adapter fails
  loudly (`UnexpectedLayout`) rather than guessing.
- **Account risk.** Automation is visible to X and may trigger challenges or
  restrictions. The adapter stops and reports instead of evading them; the user
  accepts the account risk by signing in.
- **Speed and weight.** Each operation launches (or reuses) Chrome, waits for
  the page, and types with human-ish delays. It is slower than an API and needs
  a Chrome install.
- **Terms of service.** A browser session is not an authorized API integration.
  This is a single-user local client, not a hosted service; that distinction is
  what the owner accepted.
- **No metrics.** The goal is narrower than "everything x.com does".
- **Sign-in is manual and therefore not automatable.** A user who only has
  Google SSO must complete Google's check in the plain window; Herald cannot
  and should not do it for them.

## Consequences

- `ProviderId::X` is implemented; plans for X are live instead of inert.
- `herald x login` is the only visible-browser command; everything else is
  headless by default (`x.headless = true`).
- The provider ships with scripted fixtures only. The selectors must be
  re-captured from the dedicated profile before a release is trusted against
  the live site.
- Adding a capability for X (metrics, follow, delete) requires an ADR and a
  core operation, exactly like any other provider.

## When to revisit

- X offers a reasonably priced official API for single-user read/write.
- The DOM breaks often enough that maintenance exceeds the owner's tolerance.
- The owner wants metrics or any capability outside the seven methods: that is
  a new ADR, not a selector change.
