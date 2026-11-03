"""Prompt-end states on the 0.8B: does the clarify decision exist *before* the
model commits to an operation? Closes the loophole that the decision-token
probe only sees a post-commitment state."""
import json
from pathlib import Path
import numpy as np, torch
import run_wsp as wsp
from eval35 import load
OUT = Path(__file__).resolve().parent / "out"
torch.set_num_threads(8)
cases = wsp.load_cases()
tok, model = load(str(OUT / "model35"), "bf16")
texts = wsp.batch_texts(tok, cases)
layers = None
for case, text in zip(cases, texts):
    enc = tok(text, return_tensors="pt", truncation=True, max_length=1024)
    with torch.inference_mode():
        out = model(**enc, output_hidden_states=True)
    if layers is None:
        layers = [[] for _ in out.hidden_states]
    for i, s in enumerate(out.hidden_states):
        layers[i].append(s[0, -1, :].float().cpu().numpy())
    print("ok", case.id, flush=True)
np.savez_compressed(OUT / "prompt35.npz",
                    **{f"layer_{i}": np.stack(c) for i, c in enumerate(layers)},
                    gold=np.array([c.clarify for c in cases], dtype=bool))
print("wrote prompt35.npz")
