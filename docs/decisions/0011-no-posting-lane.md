# 0011: No posting lane in Herald

Date: 2026-11-07
Status: Accepted
Supersedes: [0009](0009-scoped-autopost.md) (scoped autopost for original posts)

## Context

[ADR 0009](0009-scoped-autopost.md) gave Herald one non-approval path to the
wire: original X posts to the user's own timeline, behind `allow_send` +
`allow_autopost`, a kill-switch file, UTC quiet hours, a daily budget, and a
deterministic quality floor.

That lane put tweet generation and posting inside Herald, duplicating the
functionality of the Zyquor posting pipeline. The owner's product split is
simple: Zyquor owns tweet generation and posting; Herald owns communication
routing — reading mentions and DMs, drafting replies, and sending only after
explicit human approval.

## Decision

Herald has **no posting operation, no autopost lane, and no code path that
sends without an approval**. Specifically:

- `Operation::CreatePost` is removed from the core enum. Old plans that name
  it fail closed at deserialization.
- `Target::Timeline` survives only as a legacy decode variant so approvals
  queued before the removal can still be listed and rejected. `validate_plan`
  refuses it for every operation, and every execution path that meets it
  fails closed; nothing is ever sent to it.
- `PolicyConfig` drops `AutopostPolicy`. `authorize` never returns `Allowed`
  for a send; every send is `NeedsApproval` (or `Rejected`).
- The X adapter accepts only `mention:<status id>` and `dm:<conversation id>`
  thread ids. `timeline` is rejected; a legacy queued approval can be
  rejected via `herald reject`, and approving it fails closed at
  re-resolution.
- The deterministic tweet quality scorer, the model-written post prompt, and
  `herald autopost`, `herald post exactly:`/tweet phrases, the UI compose
  panel, and `/api/post` + `/api/autopost` are removed. Tweet/post requests
  get a deterministic decline saying Herald does not post to social networks.
- Config keys for the lane (`allow_autopost`, `autopost_daily_limit`,
  `autopost_min_quality`, `autopost_quiet_hours`) and their env overrides are
  removed. Unknown-key rejection now fails closed on a stale file or
  environment.
- `[x] read_only` stays: reads keep working while every X write is refused.

## Why

- **One owner per capability.** Zyquor already has the posting pipeline,
  review queue, quality gate, scheduling, and performance loop. Herald
  carrying a second implementation only creates ambiguity about which
  behavior is authoritative.
- **Smaller safety surface.** The approval queue is the single wire path.
  There is no exception branch to widen, no daily budget to reason about,
  and no quality heuristic in the decision path.
- **Explicit rejection over silent mapping.** A post request cannot be
  confused with a reply. The rules layer declines deterministically instead
  of routing it to a send, a draft, or the model.

## Trade-offs

- Users who wrote `herald ask "post exactly: …"` must use the Zyquor
  pipeline instead; Herald will say so rather than partially guessing.
- Pending `timeline` approvals from earlier builds cannot be approved, but
  they remain listable and rejectable, so nothing is stranded.
- Config files that mention autopost keys now fail to parse until the
  owner removes them.

## Consequences

- `herald` reads X mentions, DMs, the home feed, and notifications, and
  sends replies/DMs only after `herald approve`.
- The autopost kill-switch file, daily budget counters, and quality-floor
  language disappear from the CLI, UI, config, and docs.
- Widening the X surface beyond replies and DMs, or reintroducing any
  auto-send, requires a new ADR and new per-property tests.

## When to revisit

- The Zyquor pipeline is retired and Herald is explicitly asked to absorb
  generation: a single-capability ADR, not a config flag.
- Any proposal for a non-approval send: rejected by construction; revisit
  only with an ADR that explains why a human-in-the-loop is no longer
  needed.
