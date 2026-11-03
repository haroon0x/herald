#!/usr/bin/env python3
"""Cross-validated linear decodability of the gold clarification label.

The decision-position margin shows the model's policy is operation-conditioned
(send -> false, read/draft -> true). A readout weight edit can only help if the
*correct* label is linearly decodable from the hidden state at that position,
out of sample. This script measures that with ridge regression and 5-fold CV,
overall and within the operation families where the errors actually live.
"""

from __future__ import annotations

import json
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve().parent
OUT = HERE / "out"


def load() -> tuple[np.ndarray, np.ndarray, list[str]]:
    data = np.load(OUT / "decision_hidden.npz")
    layers = sorted(data.files, key=lambda name: int(name.split("_")[1]))
    hidden = np.stack([data[name] for name in layers])  # [layers, cases, d]
    rows = json.loads((OUT / "decision_probe.json").read_text())["rows"]
    gold = np.array([row["gold_clarify"] for row in rows], dtype=bool)
    ids = [row["case_id"] for row in rows]
    return hidden, gold, ids


def ridge_fit(x: np.ndarray, y: np.ndarray, lam: float) -> tuple[np.ndarray, float]:
    # Dual form is cheaper than d x d here (n << d).
    x_mean = x.mean(axis=0)
    y_mean = y.mean()
    xc = x - x_mean
    yc = y - y_mean
    gram = xc @ xc.T
    alpha = np.linalg.solve(gram + lam * np.eye(gram.shape[0]), yc)
    w = xc.T @ alpha
    b = y_mean - x_mean @ w
    return w, float(b)


def folds(n: int, k: int, seed: int = 0) -> list[np.ndarray]:
    rng = np.random.default_rng(seed)
    order = rng.permutation(n)
    return [order[i::k] for i in range(k)]


def cv_accuracy(
    x: np.ndarray, y: np.ndarray, lam: float, k: int = 5, seed: int = 0
) -> tuple[float, np.ndarray]:
    n = len(y)
    predictions = np.zeros(n, dtype=bool)
    for test in folds(n, k, seed):
        train = np.setdiff1d(np.arange(n), test)
        w, b = ridge_fit(x[train], y[train].astype(float), lam)
        predictions[test] = (x[test] @ w + b) > 0
    return float((predictions == y).mean()), predictions


def main() -> int:
    hidden, gold, ids = load()
    print(f"cases={len(gold)} clarify_true={gold.sum()} clarify_false={(~gold).sum()}")

    # The send family and the read/draft family are the natural split.
    send_ids = [
        "send-exact",
        "send-reply-saying",
        "send-sam-late",
        "respond-noted",
        "respond-approved",
        "reply-unsubscribe",
        "reply-founder-next",
        "send-no-body",
    ]
    send_mask = np.array([case_id in send_ids for case_id in ids])
    print(f"send_family={send_mask.sum()} other={(~send_mask).sum()}")

    lams = [0.01, 0.1, 1.0, 10.0, 100.0]
    print("\noverall 5-fold CV accuracy by layer (best lambda):")
    best_layers = []
    for index in range(hidden.shape[0]):
        scores = [cv_accuracy(hidden[index], gold, lam)[0] for lam in lams]
        best = int(np.argmax(scores))
        best_layers.append((scores[best], index, lams[best]))
        if index % 4 == 0 or scores[best] >= 0.8:
            print(
                f"  hidden[{index:>2}] (decoder {index - 1:>2}): "
                f"acc={scores[best]:.3f} lam={lams[best]}"
            )
    best_layers.sort(reverse=True)
    print("\ntop layers:")
    for acc, index, lam in best_layers[:5]:
        print(f"  hidden[{index}] (decoder {index - 1}): acc={acc:.3f} lam={lam}")

    # Within the non-send family: can it tell an actionable read from an
    # underspecified one? This is the part the model currently gets wrong.
    other = ~send_mask
    print("\nwithin non-send family, 5-fold CV accuracy by layer:")
    for index in range(0, hidden.shape[0], 4):
        x = hidden[index][other]
        y = gold[other]
        scores = [cv_accuracy(x, y, lam)[0] for lam in lams]
        best = int(np.argmax(scores))
        print(
            f"  hidden[{index:>2}]: acc={scores[best]:.3f} lam={lams[best]} "
            f"(n={len(y)}, true={y.sum()})"
        )

    # Confusion matrix for the best overall layer.
    acc, index, lam = best_layers[0]
    _, predictions = cv_accuracy(hidden[index], gold, lam)
    tp = int((predictions & gold).sum())
    tn = int((~predictions & ~gold).sum())
    fp = int((predictions & ~gold).sum())
    fn = int((~predictions & gold).sum())
    print(
        f"\nbest layer {index} (decoder {index - 1}) lam={lam} acc={acc:.3f}: "
        f"tp={tp} tn={tn} fp={fp} fn={fn}"
    )

    (OUT / "decision_cv.json").write_text(
        json.dumps(
            {
                "best": {
                    "layer": index,
                    "accuracy": acc,
                    "lambda": lam,
                    "tp": tp,
                    "tn": tn,
                    "fp": fp,
                    "fn": fn,
                },
                "top_layers": [
                    {"hidden_index": i, "accuracy": a, "lambda": l}
                    for a, i, l in best_layers[:5]
                ],
            },
            indent=2,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
