#!/usr/bin/env python3
"""Doctor step 1 on the patient: localise the clarify decision in Qwen3.5-0.8B.

We are the programmer; the 0.8B is the patient. The blog's protocol is
read -> hypothesise -> intervene -> verify. This script does the *read*.

For every eval case we reconstruct the exact prefix the model produced, cut it at
`"needs_clarification": `, and record at that decision token:

  * the true/false logit margin (what the patient is about to say),
  * the residual stream at every layer (where the decision lives).

Then it tests whether the gold clarify label is recoverable from those states,
linearly and with a quadratic expansion. The quadratic test matters: a policy
that is not linearly decodable may still be present, and if it is, a rank-one
weight edit is the wrong instrument but a different edit is not.

Writes out/decide35.json and out/decide35.npz.
"""

from __future__ import annotations

import json
import re
from pathlib import Path

import numpy as np
import torch

import run_wsp as wsp
from eval35 import load

HERE = Path(__file__).resolve().parent
OUT = HERE / "out"
MARKER = '"needs_clarification": '


def prefix_for(raw: str, fallback_op: str) -> str:
    index = raw.find(MARKER)
    if index >= 0:
        return raw[: index + len(MARKER)]
    match = re.search(r'"operation"\s*:\s*"([a-z_]+)"', raw)
    op = match.group(1) if match else fallback_op
    return f'{{"operation": "{op}", "needs_clarification": '


def main() -> int:
    torch.set_num_threads(8)
    cases = wsp.load_cases()
    baseline = json.loads((OUT / "baseline35.json").read_text())["results"]
    by_result = {row["case_id"]: row for row in baseline}

    tok, model = load(str(OUT / "model35"), "bf16")
    true_ids = tok.encode(" true", add_special_tokens=False)
    false_ids = tok.encode(" false", add_special_tokens=False)
    print(f"true_ids={true_ids} ({tok.decode(true_ids)!r})")
    print(f"false_ids={false_ids} ({tok.decode(false_ids)!r})")
    if len(true_ids) != 1 or len(false_ids) != 1:
        print("WARNING: decision is not a single token on this tokenizer")
    true_id, false_id = true_ids[0], false_ids[0]

    texts = wsp.batch_texts(tok, cases)
    rows = []
    states_by_layer: list[list[np.ndarray]] = []

    for case, text in zip(cases, texts):
        raw = by_result[case.id]["raw"] or ""
        prefix = prefix_for(raw, case.operation)
        full = text + prefix
        enc = tok(full, return_tensors="pt", truncation=True, max_length=1024)
        with torch.inference_mode():
            out = model(**enc, output_hidden_states=True)
        logits = out.logits[0, -1].float()
        hidden = out.hidden_states
        if not states_by_layer:
            states_by_layer = [[] for _ in hidden]
        for layer_index, state in enumerate(hidden):
            states_by_layer[layer_index].append(
                state[0, -1, :].float().cpu().numpy()
            )
        rows.append(
            dict(
                case_id=case.id,
                gold_clarify=case.clarify,
                gold_operation=case.operation,
                emitted_operation=by_result[case.id]["operation"],
                raw=raw.strip(),
                prefix=prefix,
                n_tokens=int(enc["input_ids"].shape[1]),
                margin=float(logits[true_id] - logits[false_id]),
                p_true=float(torch.softmax(logits[[true_id, false_id]], dim=0)[0]),
            )
        )

    hidden = [np.stack(chunk) for chunk in states_by_layer]
    gold = np.array([row["gold_clarify"] for row in rows], dtype=bool)

    print("\nmargin by gold label (true - false):")
    for value in (False, True):
        group = [r["margin"] for r in rows if r["gold_clarify"] is value]
        print(f"  clarify={value}: n={len(group)} mean={np.mean(group):+.3f} "
              f"median={np.median(group):+.3f}")

    print("\nper-layer separation of gold label at the decision token:")
    best = (0.0, -1)
    separations = []
    for index, states in enumerate(hidden):
        no = states[~gold]
        yes = states[gold]
        diff = yes.mean(axis=0) - no.mean(axis=0)
        scale = np.sqrt(0.5 * (yes.var(axis=0).mean() + no.var(axis=0).mean()))
        sep = float(np.linalg.norm(diff) / (scale + 1e-6))
        separations.append(sep)
        if sep > best[0]:
            best = (sep, index)
        print(f"  hidden[{index:>2}] (decoder {index - 1:>2}): {sep:6.2f}")
    print(f"\nmost separated: hidden[{best[1]}] (decoder {best[1] - 1}) at {best[0]:.2f}")

    (OUT / "decide35.json").write_text(
        json.dumps(
            dict(rows=rows, separations=separations, best_hidden=best[1]), indent=2
        )
    )
    np.savez_compressed(
        OUT / "decide35.npz",
        **{f"layer_{i}": s for i, s in enumerate(hidden)},
        gold=gold,
    )
    print(f"wrote {OUT / 'decide35.json'} and decide35.npz  ({len(rows)} cases)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
