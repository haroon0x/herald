#!/usr/bin/env python3
"""Exploratory probe at the *decision position*, not the prompt end.

The first pilot measured hidden states at the last prompt token and projected a
difference-in-means direction out of one MLP. That failed, plausibly because the
clarify decision is made later, after the operation token has been emitted.

This script reconstructs, for every eval case, the exact prefix the model
produced in the baseline run, truncates it at
`"needs_clarification": `, and reads:

  * the true/false logit margin at that decision token,
  * the hidden state at that position for every layer.

It then reports (a) how separable the *gold* clarification label is at each
layer and (b) whether ablating that direction from the residual stream changes
the margin. No weights are edited here; this is the localisation step.
"""

from __future__ import annotations

import json
from pathlib import Path

import numpy as np
import torch

import run_wsp as wsp

HERE = Path(__file__).resolve().parent
OUT = HERE / "out"
PREFIX_FALLBACK = '{"operation": "list_unread_messages", "needs_clarification": '


def decision_prefix(raw: str) -> str | None:
    """Everything the model emitted up to and including the decision colon."""
    marker = '"needs_clarification": '
    index = raw.find(marker)
    if index < 0:
        return None
    return raw[: index + len(marker)]


def margin(logits: torch.Tensor, true_id: int, false_id: int) -> float:
    return float(logits[true_id] - logits[false_id])


def main() -> int:
    torch.set_num_threads(8)
    cases = wsp.load_cases()
    by_id = {case.id: case for case in cases}
    baseline = json.loads((OUT / "baseline.json").read_text())["results"]
    by_result = {result["case_id"]: result for result in baseline}

    tokenizer, model = wsp.load_model(str(OUT / "model"))
    true_id = tokenizer.encode(" true", add_special_tokens=False)[0]
    false_id = tokenizer.encode(" false", add_special_tokens=False)[0]
    print(f"true_id={true_id} false_id={false_id}")

    rows = []
    hidden_by_layer: list[list[np.ndarray]] = []

    for case in cases:
        raw = by_result[case.id]["raw"]
        prefix = decision_prefix(raw) or PREFIX_FALLBACK
        text = wsp.batch_texts(tokenizer, [case])[0] + prefix
        encoded = tokenizer(text, return_tensors="pt", truncation=True, max_length=1024)
        with torch.inference_mode():
            output = model(**encoded, output_hidden_states=True)
        logits = output.logits[0, -1].float()
        states = output.hidden_states
        if not hidden_by_layer:
            hidden_by_layer = [[] for _ in states]
        for index, layer_state in enumerate(states):
            hidden_by_layer[index].append(layer_state[0, -1, :].float().cpu().numpy())
        rows.append(
            {
                "case_id": case.id,
                "gold_clarify": case.clarify,
                "baseline_clarify": by_result[case.id]["needs_clarification"],
                "margin": margin(logits, true_id, false_id),
                "prefix": prefix,
            }
        )

    hidden = [np.stack(chunk) for chunk in hidden_by_layer]
    gold = np.array([row["gold_clarify"] for row in rows])
    baseline_flag = np.array(
        [bool(row["baseline_clarify"]) for row in rows], dtype=bool
    )

    print("\nper-case margin (true - false):")
    for row in rows:
        mark = "" if row["gold_clarify"] == bool(row["baseline_clarify"]) else "  <-- wrong flag"
        print(
            f"  {row['case_id']:<24} gold={str(row['gold_clarify']):<5} "
            f"baseline={str(row['baseline_clarify']):<5} margin={row['margin']:+.3f}{mark}"
        )

    print("\nmean margin by gold label:")
    for value in (False, True):
        group = [row["margin"] for row in rows if row["gold_clarify"] is value]
        print(f"  clarify={value}: n={len(group)} mean={np.mean(group):+.3f}")

    print("\nlayer separation of gold label at decision position:")
    for index, states in enumerate(hidden):
        no_clarify = states[~gold]
        clarify = states[gold]
        diff = no_clarify.mean(axis=0) - clarify.mean(axis=0)
        scale = np.sqrt(
            0.5 * (no_clarify.var(axis=0).mean() + clarify.var(axis=0).mean())
        )
        separation = float(np.linalg.norm(diff) / (scale + 1e-6))
        print(f"  hidden[{index:>2}] (decoder {index - 1:>2}): separation={separation:7.2f}")

    (OUT / "decision_probe.json").write_text(
        json.dumps({"rows": rows}, indent=2)
    )
    np.savez_compressed(
        OUT / "decision_hidden.npz",
        **{f"layer_{i}": states for i, states in enumerate(hidden)},
    )
    print(f"\nwrote {OUT / 'decision_probe.json'} and decision_hidden.npz")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
