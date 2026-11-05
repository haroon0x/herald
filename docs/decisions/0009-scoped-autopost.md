# 0009: Scoped autopost for original posts

Date: 2026-11-04
Status: Accepted
Amends: [0005](0005-approval-queue-and-idempotency.md) (the approval rule) for
one narrow lane.

## Context

[ADR 0005](0005-approval-queue-and-idempotency.md) makes every outbound action
stop at a durable approval. That is the right default for replies and DMs: a
wrong reply reaches a person, and a wrong DM reaches a person privately.

Original timeline posts are different. The owner's goal is to publish short
updates from Herald without a two-step flow, and the worst case of a bad post
is a public post on the owner's own timeline, which the owner can delete. The
deterministic quality scorer ported from Zyquor already exists to reject
low-quality text.

The risk to control is not "a post exists"; it is **scope creep**: an exception
that quietly grows into replies, DMs, or unattended loops.

## Decision

Herald has exactly one path to the wire without a per-action approval: an
original post to the user's own timeline (`Operation::CreatePost`,
`Target::Timeline`).

Every rail must pass, or the action falls back to the normal approval queue:

1. `[policy] allow_send = true` **and** `[policy] allow_autopost = true`
   (both default false).
2. The autopost kill-switch file `<data-dir>/autopost.disabled` does not exist.
   `herald autopost off` writes it; `herald autopost on` removes it.
3. `now` is outside the configured UTC quiet window
   (`autopost_quiet_hours = "HH:MM-HH:MM"`, wrap-around supported). A malformed
   window is a config error, never "no quiet hours".
4. Today's remaining budget is greater than zero. The cap
   (`autopost_daily_limit`, default 5, 1..=1000) counts every `create_post`
   action that reached approval or execution since midnight UTC — manual and
   autonomous alike — so the lane cannot be used to exceed the cap.
5. The deterministic quality score of the body is at least
   `autopost_min_quality` (default 7 of 10).

If the configured provider fails and Herald falls back to the local quantized
model, autopost is disabled for that run even when every other rail passes: the
post is queued for approval. A fallback model never posts autonomously.

A model-drafted post is still an original timeline post: it enters this lane
only when every rail passes, and its text is scored like any other post. Drafts
are capped at 280 characters and are never truncated.

The lane executes through the same `prepare_reply` → `send_reply` →
verification path and the same state machine as an approved send. Audit details
are prefixed `autopost:`. A failed verification leaves the action `uncertain`
exactly like a manual send; it never becomes success.

DMs and replies to mentions **never** autopost. `authorize` grants `Allowed`
only for a timeline send; every other send returns `NeedsApproval`.

## Why these rails

- **Two switches, not one.** `allow_send` is the general outbound switch. A
  second key means enabling approvals never silently enables autonomous
  posting.
- **The kill switch is a file.** It is immediate, scriptable, and independent
  of config parsing. `off` works even if the config file is broken.
- **A UTC bucket.** Daily budgets need one unambiguous day boundary; UTC avoids
  DST and travel surprises.
- **Counting manual posts too.** Otherwise the cap measures only the lane and
  the account can post far more than the owner configured.
- **A deterministic quality floor.** It is testable, has no model in the loop,
  and fails closed. A model could still be asked to draft, but the score gate
  is local code.
- **Quiet hours fail closed.** Outside a valid window there is no lane.

## Trade-offs

- A perfectly good post can wait for approval because one rail failed (quiet
  hours, budget, floor). That is intentional: the user sees the queue, not a
  silent drop.
- The quality scorer is a heuristic. It rejects some acceptable posts; the
  manual approval path still sends them.
- The exception exists in `authorize`, the most safety-sensitive function in
  the codebase. It is kept as one narrow branch with a test per rail.

## Consequences

- `herald autopost status` reports configured state, kill switch, quiet hours,
  used/remaining budget, floor, and the effective send path.
- `herald ask "post exactly: …"` may publish without an approval only when
  every rail holds; otherwise it queues an approval like any other send.
- `create_post` is a core operation with its own `operation_name`, eval cases,
  and policy tests.
- Widening the lane (replies, DMs, threads, scheduling, a model-written body
  without review) requires a new ADR and new per-rail tests. It is not a config
  flag.

## When to revisit

- The owner wants scheduled or recurring posts: that is a new failure mode
  (a daemon that posts while unattended), not a new rail.
- The quality floor proves too coarse in real use: adjust the number, not the
  gate.
- Any proposal to autopost a reply or a DM: rejected by construction; revisit
  only with an ADR that explains why a human-in-the-loop is no longer needed.
