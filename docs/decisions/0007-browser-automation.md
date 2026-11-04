# 0007: Browser automation is not the provider layer

Date: 2026-11-04
Status: Accepted — the X adapter is the constrained exception in [0008](0008-x-browser-adapter.md).

## Context

Browser automation is the obvious way to reach a webmail UI that has no API,
and Playwright is the best-known tool for it. Users reasonably ask whether
Herald can drive a browser, and what the Rust equivalent is.

The honest options:

| Option | Model | Trade-off |
| --- | --- | --- |
| `chromiumoxide` | Async Chrome DevTools Protocol client | Closest Rust analogue to Playwright; no Node runtime; full page control; still a browser. |
| `fantoccini` | W3C WebDriver client | Works with geckodriver/chromedriver; mature protocol; needs a separate driver and browser install. |
| `thirtyfour` | Async WebDriver client | Good ergonomics; same driver dependency. |
| `headless_chrome` | Synchronous CDP client | Simple for scripts; blocking API and less maintained. |
| Playwright (Node) | Browser automation via a Node sidecar | Excellent tooling; adds a Node runtime and a second process to a single-binary local app. |
| IMAP + SMTP | Mail protocols | No browser, no captcha, no DOM, stable, testable; needs credentials and provider setup. |

The provider question is not "which browser library" but "which interface is
stable enough to bet a user's outbox on".

## Decision

Herald v1 does not ship browser automation. Its providers are protocol or
filesystem adapters:

- `maildir` reads a local mailbox and sends through SMTP.
- `fake` is a deterministic test/demo harness.
- `gmail` and `x` are closed enum variants with no adapter; plans are inert.

The next real provider is IMAP (read) reusing SMTP (send), not a browser. That
keeps the provider testable with a fake server, keeps credentials out of a
browser profile, and avoids captcha/2FA/DOM churn.

If a future provider genuinely has no protocol or API and browser automation is
the only path, the constraint is `chromiumoxide` with:

- a dedicated Herald browser profile, never the user's everyday profile;
- a human sign-in, with credentials entered by the user, never read from the
  profile or extracted as cookies;
- a capability surface limited to the same typed operations (`list`, `thread`,
  `prepare`, `send`, `find_sent`), never "click anything" or "run JavaScript";
- a hard stop and report on captcha, login challenge, account warning, or
  unexpected DOM;
- the same approval gate: a browser provider still cannot send without
  `herald approve`.

Playwright and WebDriver are rejected for the MVP because they add a runtime
and a driver, weaken the single-binary install, and make the safety properties
harder to test. `chromiumoxide` is the documented alternative if the constraint
changes.

## Consequences

- Gmail/Outlook users need a Maildir sync (mbsync, OfflineIMAP) or wait for
  IMAP. This is a real adoption cost, documented in
  [providers.md](../providers.md) and [faq.md](../faq.md).
- The provider port stays implementation-compatible with an IMAP adapter: it
  already exposes stable ids, threads, read, prepare, send, and find-sent.
- No Node, no browser download, no WebDriver in the default install.
