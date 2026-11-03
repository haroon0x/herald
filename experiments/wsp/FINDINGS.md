# Why the blog's result does not transfer to Herald — and what does

Date: 2026-11-03. All numbers below are from runs in `experiments/wsp/out/`.
Models: `Qwen3-0.6B` (`out/model`) and `Qwen3.5-0.8B` (`out/model35`, the
model family the blog edited).

## 1. The blog has two roles, and we gave both to the small model

| role | who does it in the blog | what it requires |
| --- | --- | --- |
| **patient** | Qwen3.5-0.8B — edited 7.9% → 48.1% on math | an ability that already exists, blocked |
| **doctor** | GPT-6 Astra — reads tensors, hypothesises, writes the edit, verifies | mechanistic world-model of the network + long-horizon tool use |

The pilot asked the 0.6B to be the patient *and* the doctor. That is the
category error behind most of the confusion in the earlier work.

## 2. The doctor bench: the small model recites the framework but cannot apply it

`doctor_bench.py`, 11 core items, 4 controls, chance 2.75/11.

| model | recite (6 items) | apply (5 items) | controls |
| --- | --- | --- | --- |
| Qwen3.5-0.8B | **6/6** | **0/5** | 3/4 |
| Qwen3-0.6B | — | — | **0/4** (parser collapses) |

The 0.8B answers every textbook question correctly — correlation vs causation,
what `down_proj` feeds, why below-majority probe accuracy means no linear
signal, that an edit removed an obstacle rather than adding an ability. It then
gets **every single application item wrong**, in the same direction as the pilot:

| item | correct | 0.8B answer |
| --- | --- | --- |
| obstacle or missing policy? | missing policy | *an obstacle hiding an ability* |
| dropped required field: weights or decoding? | decoding | *weight-level policy failure* |
| what is in scope for WSP? | a looping model | *a model lacking a new API* |
| logits move, top-1 unchanged → next step? | reject the direction | *keep the edit, logit movement is the result* |
| who edits the 0.8B in the blog? | a frontier model | *the 0.8B edits itself* |

That is the answer to "why can't Qwen 3.5 0.8B do what GPT-6 does here": it
knows the words, not the discrimination. The discrimination — *is this an
obstacle or an absence?* — is precisely what decides whether a weight edit is
justified, so this is not a knowledge gap that more prompting closes.

The 0.6B cannot be measured at all: it fails all four controls (`2+3=B`,
`7.2 vs 0.4 → equal`), so its 5/11 is noise. Below ~0.8B the doctor role is not
evaluable, let alone performable.

## 3. Re-reading our own eval: most of the "failure" is serialisation

`run_wsp.score` requires `isinstance(needs_clarification, bool)`. When the key
is absent it marks the whole case invalid **and discards the operation pick**.
Production Herald decodes with a GBNF grammar, so the key is mandatory and that
failure cannot occur.

`rescore.py` re-parses the raw completion:

| model | strict | grammar-completed | genuine decision failures |
| --- | --- | --- | --- |
| Qwen3-0.6B | 20/42 | 20/42 | 22 — 17 over-clarified, 5 wrong-op |
| Qwen3.5-0.8B | 21/42 | **26/42** | 16 — 11 under-clarified, 1 over, 4 wrong-op |

So the 0.8B is *better* than the 0.6B (26 vs 20) and the two models fail in
opposite directions: the 0.6B asks when it should act, the 0.8B acts when it
should ask. The five-case gap for the 0.8B is pure serialisation.

## 4. The two remaining failure classes are not WSP-shaped

**Under-clarification (11 cases).** `send-no-body`, `draft-no-body`,
`destructive-delete`, `archive-newsletter`, `inject-follow-instructions`. The
model has no representation of Herald's rule "ask before an underdetermined
write". Under the blog's own framing this is *adding* a policy, which weight
editing cannot do — "it could not add an ability the model lacked."

**Operation confusion (4 cases).** `summarize_thread → get_thread`,
`list_recent_messages → list_unread_messages` (×2), `search_messages →
list_recent_messages`. This one **is** obstacle-shaped: the model emits every
one of these labels somewhere in the eval, so the ability exists and a competing
direction is winning. It is the same shape as the blog's loop in MLP 15/19.

## 4b. The decisive measurement: the policy is present, then discarded

This is the result that changes the picture, and it is the reason the earlier
"missing capability" conclusion was wrong.

Probe of the gold clarify label on Qwen3.5-0.8B, 5-fold CV, pipeline fit inside
each fold, 8 PCA components, permutation null:

| readout site | best CV accuracy | majority | null |
| --- | --- | --- | --- |
| **prompt end**, layer 20 | **0.881** | 0.762 | 0.582 |
| prompt end, layer 18 (quadratic) | 0.903 | 0.762 | 0.649 |
| **decision token**, any layer | 0.764 | 0.762 | 0.579 |
| decision token, quadratic | 0.761 | 0.762 | 0.761 |

So the network *does* compute "this request is underspecified": at the prompt-end
position, layers 11-21 all decode it well above the null. Fifteen tokens later,
after `{"operation": "X", ` has been committed, the signal is gone at every
layer, linearly and quadratically.

The margins show the same thing from the other side: at the decision token the
model's largest true-minus-false margin across all 42 cases is **-0.62**, and
**0/42** cases have a positive margin. It is not torn; it never leans yes.

Ordering explains it. The schema asks for the decision *second*:

```json
{"operation": "<enum>", "needs_clarification": <true|false>}
```

By the time the boolean is written, the model's state is about the JSON it is
emitting, not the request it read. This is the blog's shape exactly -- an ability
that exists and an obstacle that keeps it from the output -- and unlike the
earlier framing it *is* a legitimate target: the information is there to be
routed, not to be invented.

It also predicts the two models' opposite failure modes. Qwen3-0.6B is full
attention at every layer and over-clarifies (17 cases); Qwen3.5-0.8B is hybrid,
with full attention only at layers 3/7/11/15/19/23 and linear attention
compressing the rest, and under-clarifies (11 cases).

Prediction under test in `prompt_reorder.py`: move `needs_clarification` to the
front of the schema and the decision lands at the prompt end, where the
representation lives. A control prompt keeps the original order with the same
rule text, so a win can be attributed to position rather than wording.

## 4c. Causal test: the obstacle is the field order

Prediction: put `needs_clarification` first and the decision lands at the prompt
end, where the representation exists. Control: same rule text, original order, so
a win cannot be blamed on wording.

| run | prompt | clarify recall | false positives | total (semantic) |
| --- | --- | --- | --- | --- |
| `baseline35` | original | **0/10** | 1/32 | 26/42 |
| `reorderctl35` | same new rule text, **original order** | **0/10** | 0/32 | — |
| `reorder35` | flag first | **10/10** | 16/32 | 26/42 |

Wording alone changes nothing (control is 0/10). Moving the field changes
everything: every clarification the model should make, it makes. The errors flip
from under- to over-clarifying, which is why the total stays at 26 -- the model
can now *make* the decision but has not *calibrated* it.

## 4d. The calibration is not recoverable from the model's own confidence

Threshold sweep on the true/false margin at the flag-first position:

- mean margin: clarify `+0.138`, no-clarify `-0.207`
- **separation AUC 0.634**
- every threshold scores at or below 26/42; the best is "never clarify"

So the decision is now made, and its confidence carries almost no information.
This is the same wall the earlier bias sweep hit: a scalar on the output cannot
supply a decision boundary.

## 4e. The representation beats the generation

The probe is a better decision surface than the boolean the model writes. Gating
clarification on the out-of-fold probe probability, and otherwise taking the
operation the model produced:

| policy | score |
| --- | --- |
| baseline 0.8B strict | 21/42 |
| baseline 0.8B semantic | 26/42 |
| model with flag-first schema | 26/42 |
| **out-of-fold readout gate, layer 20** | **32/42** (tp=8, fp=2, fn=2) |

Out-of-fold AUC 0.906. Caveat: the threshold is chosen on the same out-of-fold
predictions, so 32 is mildly optimistic; the AUC is the more robust number.

This is the read half of WSP doing real work, and it is consistent with the blog's
own emphasis: the successful examples are all reads, and Experiment 7 (sycophancy)
failed precisely because the property was not readable. For Herald the honest
conclusion is that the weights are more useful read than written.

## 4f. Calibrating it back: the shippable result

The flag-first schema has recall but no precision. The rule text was the problem:
written as "clarify when the target is not identified", it also fires on "check my
mail", which is a complete request. `prompt_cal.py` states what each operation
needs, adds an explicit do-not-clarify list, and pairs contrastive examples.

| run | strict | grammar-completed | clarify recall | false pos |
| --- | --- | --- | --- | --- |
| `baseline35` — original schema | 21 | 26 | 0/10 | 1/32 |
| `reorderctl35` — new rule text, **original order** | 14 | 27 | 0/10 | 0/32 |
| `reorder35` — flag first, loose rule | 26 | 26 | 10/10 | 16/32 |
| **`cal35` — flag first, specified rule + examples** | **31** | **31** | 4/10 | 1/32 |
| readout gate, out-of-fold (ceiling, needs hooks) | — | 32 | 8/10 | 2/32 |

The control is the key row: identical wording in the original order still has
**0/10** recall, and even scores *worse* strictly (14) than the baseline. So the
rule text alone does nothing for clarification; the field order is what makes the
decision possible, and the rule text is what makes it precise.

31/42 is a pure prompt change -- no hooks, no weight edits, no custom inference.
It is +10 over the baseline strict score and +5 over the baseline once the
grammar-forced field is accounted for. The readout gate reaches only one case
more, so the prompt now captures nearly everything the probe knew.

### Verified against the shipped prompt

The prompt was ported to `crates/herald-core/src/intent.rs` and then extracted
back out of the Rust source (`prompt_shipped.py`) and re-run, so the number below
is the one the binary will produce, not a Python lookalike:

| run | strict | clarify recall | false pos |
| --- | --- | --- | --- |
| `shipped35` -- prompt read from `intent.rs` | **32/42** | 4/10 | 1/32 |

One of its ten failures is a hallucinated enum (`delete_reply` on
`destructive-delete`), which the production grammar rejects outright; the other
nine are ordinary decision errors. The remaining misses are 5 clarification
cases (missing body, bulk/destructive, off-topic), 3 operation picks, and 1
spurious question -- no shared mechanism, which is why the tuning stops here
rather than adding examples for the exact failing cases. That would fit the
42-case set, not the problem.

## 5. What was methodologically missing

The pilot went correlational-direction → weight edit in one step. The blog's
method is causal. Missing step: **activation patching** — take the residual at
the decision position from a run that picks the right label, patch it into a run
that picks the wrong one, and see if the label flips. If patching does not flip
it, no weight edit at that site can work, and the direction is not causal. That
one check would have saved the entire `alpha 1/2/5/10` sweep.

## 6. What to do instead

1. **Doctor stays offline.** WSP is a *compile-time* step: a frontier model (or
   a strong local model with tools) produces the edit once; the shipped artifact
   is the edited checkpoint. This is fully compatible with local-first inference
   — no message content leaves the machine at runtime. It also means we do not
   need the 0.8B to be the doctor.
2. **Target the obstacle-shaped failure, not the policy gap.** Operation
   confusion, not clarification.
3. **Patch before editing.** Establish causality at the decision token, then
   write the edit into the weights that produce that activation, then verify on
   held-out cases.
4. **Fix the eval before optimising against it.** Report strict *and*
   grammar-completed, and score the injection cases separately — right now a
   `send_reply` on `inject-follow-instructions` costs the same as a formatting
   slip.

## Reproduction

```bash
cd experiments/wsp
.venv/bin/python eval35.py --name baseline35 --dtype bf16     # 42-case baseline
.venv/bin/python rescore.py out/baseline35.json out/baseline.json
.venv/bin/python doctor_bench.py --model out/model35 --dtype bf16 --name doctor35
.venv/bin/python doctor_bench.py --model out/model   --dtype fp32 --name doctor06
```
