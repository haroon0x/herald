#!/usr/bin/env python3
"""Read the patient's own uncertainty about the operation.

The doctor's read step. We ask: before the model commits to an operation, is it
*less certain* exactly on the instructions that need clarification? If yes, the
knowledge required to gate clarification is already inside the network -- it just
never reaches the boolean, because the boolean is emitted after the operation is
already committed and defaults to false.

That would be a read-side WSP result: no weights edited, but the model's internal
state is used to drive a decision it cannot make explicitly.

For each case we score all seven operations as continuations of the same prefix
and take the normalised distribution over them. Then we test whether the
resulting margin/entropy separates the instructions that should be clarified
from those that should not.

Usage:
    python uncertainty.py --out out/uncertainty35.json
"""

from __future__ import annotations

import argparse
import json
import math
from pathlib import Path

import numpy as np
import torch

import run_wsp as wsp
from eval35 import load

HERE = Path(__file__).resolve().parent
OUT = HERE / "out"
PREFIX = '{"operation": "'


def op_continuation(tok, prefix_ids: list[int], text: str, op: str) -> tuple[list[int], list[int]]:
    """Token ids for `op` after `prefix`, plus the ids of `prefix` itself."""
    full = tok(text + PREFIX + op, add_special_tokens=False).input_ids
    # Longest common prefix keeps us honest about BPE merges at the boundary.
    k = 0
    while k < min(len(full), len(prefix_ids)) and full[k] == prefix_ids[k]:
        k += 1
    return prefix_ids[:k], full[k:]


def score_operations(tok, model, text: str, ops: list[str]) -> dict:
    prefix_ids = tok(text + PREFIX, add_special_tokens=False).input_ids
    seqs = []
    spans = []
    for op in ops:
        shared, cont = op_continuation(tok, prefix_ids, text, op)
        ids = shared + cont
        seqs.append(ids)
        spans.append((len(shared), len(ids)))

    # Left-pad so every row ends at the same position, then ask the model for
    # logits only on the last k positions. The full logits tensor would be
    # 7 x 1000 x 248320 floats (~7 GB); this keeps it at a few MB.
    width = max(len(s) for s in seqs)
    k = max(end - start for start, end in spans) + 1  # +1 for the logit that predicts the first token
    pad = tok.pad_token_id or tok.eos_token_id
    input_ids = torch.full((len(seqs), width), pad, dtype=torch.long)
    attention = torch.zeros((len(seqs), width), dtype=torch.long)
    for row, ids in enumerate(seqs):
        input_ids[row, width - len(ids):] = torch.tensor(ids)
        attention[row, width - len(ids):] = 1

    with torch.inference_mode():
        try:
            out = model(
                input_ids=input_ids,
                attention_mask=attention,
                logits_to_keep=k,
            )
            tail = out.logits.float()
        except TypeError:
            out = model(input_ids=input_ids, attention_mask=attention)
            tail = out.logits[:, -k:].float()
    log_probs = torch.log_softmax(tail, dim=-1)

    scores = []
    for row, ((start, end), ids) in enumerate(zip(spans, seqs)):
        cont_len = end - start
        cont = ids[start:end]
        total = 0.0
        for j, token in enumerate(cont):
            # Logits for the j-th continuation token sit at tail index
            # (k - cont_len + j - 1); j == 0 predicts from the shared prefix.
            index = k - cont_len + j - 1
            if index < 0:
                raise RuntimeError("logits_to_keep too small for this continuation")
            total += float(log_probs[row, index, token])
        scores.append(total)
    scores = np.array(scores)
    probs = np.exp(scores - scores.max())
    probs /= probs.sum()
    order = np.argsort(-probs)
    entropy = float(-(probs * np.log(np.clip(probs, 1e-12, None))).sum() / math.log(2))
    return dict(
        probs={op: float(probs[i]) for i, op in enumerate(ops)},
        top1=ops[order[0]],
        top2=ops[order[1]],
        margin=float(probs[order[0]] - probs[order[1]]),
        entropy=entropy,
        scores=scores.tolist(),
    )


def auc(scores: np.ndarray, labels: np.ndarray) -> float:
    """Probability a positive outranks a negative (ties count half)."""
    pos = scores[labels]
    neg = scores[~labels]
    if len(pos) == 0 or len(neg) == 0:
        return float("nan")
    greater = (pos[:, None] > neg[None, :]).sum()
    equal = (pos[:, None] == neg[None, :]).sum()
    return float((greater + 0.5 * equal) / (len(pos) * len(neg)))


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--name", default="uncertainty35")
    args = parser.parse_args()

    torch.set_num_threads(8)
    cases = wsp.load_cases()
    ops = list(wsp.OPERATIONS)
    tok, model = load(str(OUT / "model35"), "bf16")
    texts = wsp.batch_texts(tok, cases)

    rows = []
    for case, text in zip(cases, texts):
        scored = score_operations(tok, model, text, ops)
        rank = sorted(scored["probs"], key=lambda o: -scored["probs"][o])
        rows.append(
            dict(
                case_id=case.id,
                gold_clarify=case.clarify,
                gold_operation=case.operation,
                top1=scored["top1"],
                top2=scored["top2"],
                margin=scored["margin"],
                entropy=scored["entropy"],
                p_gold=scored["probs"][case.operation],
                gold_rank=rank.index(case.operation) + 1,
                probs=scored["probs"],
            )
        )
        print(
            f"{case.id:<26} clarify={str(case.clarify):<5} gold={case.operation:<24} "
            f"top1={scored['top1']:<24} margin={scored['margin']:.3f} H={scored['entropy']:.2f} "
            f"rank={rows[-1]['gold_rank']}",
            flush=True,
        )

    gold = np.array([r["gold_clarify"] for r in rows], dtype=bool)
    margin = np.array([r["margin"] for r in rows])
    entropy = np.array([r["entropy"] for r in rows])
    gold_prob = np.array([r["p_gold"] for r in rows])
    correct_op = np.array([r["gold_rank"] == 1 for r in rows])

    print(f"\noperation top-1 accuracy: {correct_op.sum()}/{len(rows)}")
    print(f"AUC(small margin -> clarify):   {auc(-margin, gold):.3f}")
    print(f"AUC(high entropy -> clarify):   {auc(entropy, gold):.3f}")
    print(f"AUC(low p_gold -> clarify):     {auc(-gold_prob, gold):.3f}")
    print("\nmean by gold label:")
    for value in (False, True):
        mask = gold == value
        print(
            f"  clarify={value}: n={mask.sum():>2} "
            f"margin={margin[mask].mean():.3f} H={entropy[mask].mean():.2f} "
            f"p_gold={gold_prob[mask].mean():.3f} op_acc={correct_op[mask].mean():.2f}"
        )

    # The gate: clarify when the operation distribution is not decisive.
    best = None
    print("\ngate sweep (clarify if margin < t):")
    for t in np.linspace(0.0, 0.9, 19):
        pred = margin < t
        tp = int((pred & gold).sum())
        fp = int((pred & ~gold).sum())
        fn = int((~pred & gold).sum())
        # A clarification is the correct action when gold is true; when gold is
        # false, gating costs an unnecessary question but the operation pick is
        # still right, so we score against semantic eval accuracy.
        acc = (correct_op & ~pred & ~gold).sum() + (pred & gold).sum()
        acc = int(acc)
        if best is None or acc > best[1]:
            best = (t, acc, tp, fp, fn)
        print(f"  t={t:.2f}  clarify_calls={int(pred.sum()):>2} tp={tp:>2} fp={fp:>2} "
              f"fn={fn:>2}  score={acc}/42")

    print(f"\nbest gate t={best[0]:.2f} -> {best[1]}/42 "
          f"(tp={best[2]} fp={best[3]} fn={best[4]})")
    (OUT / f"{args.name}.json").write_text(
        json.dumps(dict(rows=rows, best_gate=best), indent=2)
    )
    print(f"wrote {OUT / (args.name + '.json')}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
