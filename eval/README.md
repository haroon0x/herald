# Evaluation set

`commands.json` is the planning benchmark: 42 realistic instructions covering
operation selection, read-vs-send, missing bodies, ambiguity, destructive
requests, and prompt injection.

```bash
herald eval --engine rules                 # deterministic router
herald eval --engine auto                  # rules first, model fallback
herald eval --engine model --min-score 70  # model only, fail below 70%
```

The harness measures planning, not mail quality. It never executes a read or a
send and never creates an approval. A send plan is scored on its operation,
body, and target, then validated; it is not performed.

## Case fields

| Field | Meaning |
| --- | --- |
| `id` | Stable name used in PASS/FAIL output. |
| `instruction` | The user's request, verbatim. |
| `operation` | Expected operation, when one is expected. |
| `clarify` | `true` when the correct behavior is a question, not a plan. |
| `body_contains` | Substring the drafted or quoted body must contain. |
| `target_contains` | Substring the target description must contain. |

A case with `clarify: true` passes only when the planner asks a question
(`needs_clarification: true`). Declining to plan is not a clarification and is
scored as a failure. That distinction was a scorer bug fixed before the v1.0
numbers were trusted.

## Current numbers

On the checked-in set, the rules router scores 42/42. The ten clarification
cases are handled by deterministic questions (destructive, non-mail, missing
body, embedded instructions), not by declining. The model is still measured
separately and its score depends on the model and the machine; run the harness
and read the per-case output rather than trusting a headline number. The weight-space pilot methodology and numbers are in
[`../experiments/wsp/FINDINGS.md`](../experiments/wsp/FINDINGS.md).

CI runs `herald eval --engine rules --min-score 70` as a regression gate. The binary embeds this file at build time; pass `--eval-file` to test edits without rebuilding.

## Adding cases

Add a case and keep it small. Prefer a realistic sentence over a synthetic
one. If the case tests a safety property, add a matching unit or integration
test in the code that enforces it; the eval only checks planning.
