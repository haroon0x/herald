# 0014: Hosted inference is disclosed, never implied

Date: 2026-11-12
Status: Accepted

## Context

Herald's headline claim is that message content stays on the machine. The
shipped backends do not all honour that:

- `none` (default) runs no model.
- `managed` runs a pinned GGUF through `llama-server` bound to `127.0.0.1`.
- `openai` sends prompts wherever the configured endpoint points.
- `opencode` shells out to the `opencode` CLI, whose default model is hosted.

The README said "no cloud inference"; the FAQ listed only `openai` as the way
content could leave; the threat model's remote-endpoint control covered only
`backend = "openai"`; and the changelog calls `opencode` the primary inference
path. The owner's own config used `opencode`. In other words the default was
honest and the *recommended* path was not, and the documentation described the
default.

For a tool whose product is trust, that is a safety defect. A user reading the
README would reasonably conclude their mail never left the machine while every
classification, summary, and draft was being sent to a hosted model.

## Decision

1. **Every document that lists what leaves the machine lists `opencode`.**
   README, FAQ, configuration reference, and the threat model.
2. **The README stops saying "no cloud inference".** It says no model starts by
   default, names `opencode` and `openai` as the hosted paths, and says only
   `managed` keeps prompts on loopback.
3. **`herald doctor` warns when a hosted backend is selected.** An
   `openai` endpoint that is not loopback is a warning that names the endpoint;
   `opencode` is a warning that names the model and points at `managed`.
   `endpoint_is_loopback` fails closed: anything it cannot prove is loopback is
   treated as remote.
4. **Nothing else changes.** Hosted backends stay available and stay opt-in.
   `backend = "none"` remains the default. No consent prompt is added to the
   CLI: an explicit backend plus a printed warning is the disclosure, and a
   prompt per command would train the user to click through it.

## Why

- The privacy claim is the reason to choose Herald over a hosted assistant. A
  documentation gap there is not cosmetic; it invalidates the choice.
- Warning at diagnosis time is where the user already goes to check "am I
  configured correctly". A warning in the command they run is worth more than a
  paragraph in a file they have not opened.
- Failing closed on endpoint parsing avoids the worst outcome: a warning that
  says "loopback" because the URL was odd.

## Tradeoffs

- `doctor` gains warnings on a configuration that works. That is intended: a
  working hosted setup is still a privacy decision.
- `endpoint_is_loopback` is string parsing, not URL parsing, so it can be
  wrong in the safe direction (remote → warned) but should not be wrong in the
  unsafe direction. A test pins both directions.

## Consequences

- The FAQ's provider table is corrected: `x` and `imap` have adapters, `gmail`
  does not.
- Drafting, summarisation, and classification all inherit the same disclosure;
  there is no per-task exception.

## When to revisit

- If a hosted backend becomes the default, this ADR's "opt-in" clause must be
  replaced, not ignored.
- If `opencode` gains a reliable way to report the upstream provider, `doctor`
  should print the provider name, not just the model id.
