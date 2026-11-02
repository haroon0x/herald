# 0001: Independent repo with a late convergence gate

Date: 2026-11-02
Status: Accepted

Herald starts as its own repository, not inside Zyquor.

Why:

- Zyquor is mid-migration from TypeScript to Rust. Adding a second large
  surface during migration makes failures ambiguous and slows both.
- The products have different shapes: a posting/review tool vs. a
  command-driven read/draft/send agent.
- The risk profiles differ: a bad public post vs. sending mail to the wrong
  recipient from injected instructions. Each deserves its own policy tests.

Convergence gate: when Zyquor's Rust `browser`/`automation` crates reach parity
and pass their harness, compare both browser layers and extract the better one
into a shared crate. Storage and policy types are the second candidates. Sidecar
runtime manager can move only if Zyquor ever wants local models.

Until then this repo must build, test, and ship on its own.
