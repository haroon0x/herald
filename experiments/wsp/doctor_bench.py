#!/usr/bin/env python3
"""Is a small model a viable *WSP programmer*?

The blog's workflow has two roles that are easy to conflate:

  * the **patient**  -- the network whose weights get edited (Qwen3.5-0.8B,
    edited from 7.9% to 48.1% on math);
  * the **doctor**   -- the LLM that reads the tensors, forms a mechanistic
    hypothesis, writes the edit, and verifies it (GPT-6 Astra).

Our pilot asked a 0.6B model to be both. This bench measures the doctor role
directly, with no weight access at all: every question is answerable from the
numbers, and each one is a step the blog's editor actually performs.

Four control items check that the harness is sane; if controls fail, the score
says nothing about WSP. Chance is 25%.

Usage:
    python doctor_bench.py --model out/model35 --dtype bf16 --name doctor35
    python doctor_bench.py --model out/model     --dtype fp32 --name doctor06
"""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path

import torch
from transformers import AutoModelForCausalLM, AutoTokenizer

HERE = Path(__file__).resolve().parent
OUT = HERE / "out"

SYSTEM = (
    "You are a mechanistic interpretability engineer. You read measurements from "
    "a neural network and decide what they imply. Answer with a single letter."
)

# kind: control | core.  `why` records the reasoning a competent editor uses.
ITEMS = [
    dict(
        id="ctl-arith",
        kind="control",
        q="Two measured numbers: 1.87 and 0.42. Which is larger?",
        opts=["1.87", "0.42", "they are equal", "cannot be determined"],
        answer="A",
        why="Trivial comparison; checks the harness can read numbers.",
    ),
    dict(
        id="ctl-sum",
        kind="control",
        q="What is 2 + 3?",
        opts=["5", "6", "4", "7"],
        answer="A",
        why="Trivial arithmetic control.",
    ),
    dict(
        id="ctl-order",
        kind="control",
        q="In an experiment, which comes first: forming a hypothesis, or verifying it?",
        opts=["forming the hypothesis", "verifying it", "both at once", "neither"],
        answer="A",
        why="Ordering control.",
    ),
    dict(
        id="ctl-big",
        kind="control",
        q="Which separation score is larger: 7.2 or 0.4?",
        opts=["7.2", "0.4", "equal", "cannot be determined"],
        answer="A",
        why="Comparison control using the actual units from our probe.",
    ),
    dict(
        id="core-linalg",
        kind="core",
        q=(
            "An MLP writes a vector w into the residual stream. You project the "
            "direction d out of that write, so the component of w along d becomes "
            "zero. What happens to the logit measured along d?"
        ),
        opts=["it increases", "it decreases", "it is unchanged", "it oscillates"],
        answer="B",
        why="Removing the d-component of a write reduces the readout along d.",
    ),
    dict(
        id="core-read",
        kind="core",
        q=(
            "A probe separates the gold label with score 0.4 at layer 12 and 7.2 at "
            "layer 20. Does the high score at layer 20 prove the label is causally "
            "encoded there?"
        ),
        opts=[
            "yes, separation is causal evidence",
            "no, separation is correlational until an intervention moves behaviour",
            "yes, but only above 5.0",
            "only if the sample is larger",
        ],
        answer="B",
        why="The blog's whole method distinguishes readable structure from causal structure.",
    ),
    dict(
        id="core-decode",
        kind="core",
        q=(
            "A ridge probe reaches 0.595 accuracy on a binary label. Always predicting "
            "the majority class reaches 0.762. A permutation test gives about 0.44. "
            "What follows?"
        ),
        opts=[
            "the label is linearly decodable",
            "there is no linear signal; the probe is worse than the baseline",
            "the probe needs more regularisation",
            "the label must be nonlinearly decodable",
        ],
        answer="B",
        why="Below-majority accuracy plus chance-level permutation means no linear readout.",
    ),
    dict(
        id="core-site",
        kind="core",
        q=(
            "You edit the down_proj of decoder layer 27. Which activations can that "
            "edit possibly change?"
        ),
        opts=[
            "only layer 27's own output",
            "layer 27's output and every later layer that consumes it",
            "only the final logits",
            "none, because down_proj is not on the residual path",
        ],
        answer="B",
        why="Layer 27's output is added to the residual and flows onward.",
    ),
    dict(
        id="core-alpha",
        kind="core",
        q=(
            "Scaling one edit by 10x changes nothing measurable; scaling it by 20x "
            "destroys the JSON output entirely while the task accuracy is unchanged. "
            "What is the best explanation?"
        ),
        opts=[
            "the direction is not aligned with the computation, so large scale only adds noise",
            "the layer choice is wrong",
            "the evaluation set is too small",
            "alpha is not a real parameter",
        ],
        answer="A",
        why="A causal direction moves the target monotonically; this one only degrades.",
    ),
    dict(
        id="core-obstacle",
        kind="core",
        q=(
            "A model scores 7.9% on math. After an edit removes a repeating loop, it "
            "scores 48.1%. Did the edit add a new capability?"
        ),
        opts=[
            "yes, it added the math ability",
            "no, it removed an obstacle that was hiding an ability already present",
            "unclear from the numbers",
            "yes, but only partially",
        ],
        answer="B",
        why="Exactly the blog's own claim about what weight editing can and cannot do.",
    ),
    dict(
        id="core-missing",
        kind="core",
        q=(
            "A model never asks for clarification; it always answers, because its "
            "training prior is to answer questions. Is this an obstacle hiding an "
            "ability, or a missing policy?"
        ),
        opts=[
            "an obstacle hiding an ability",
            "a missing policy, which an edit cannot install",
            "both equally",
            "neither",
        ],
        answer="B",
        why="This is the distinction the blog draws; adding a rule is out of scope.",
    ),
    dict(
        id="core-grammar",
        kind="core",
        q=(
            'A model emits {"operation": "list_unread_messages"} and drops a required '
            "boolean field. Is this a weight-level policy failure or a format failure?"
        ),
        opts=[
            "weight-level policy failure",
            "format/decoding failure",
            "both equally",
            "neither, the schema is wrong",
        ],
        answer="B",
        why="A dropped required key is a constrained-decoding problem, not a belief.",
    ),
    dict(
        id="core-scope",
        kind="core",
        q="Which target is in scope for weight-space programming as described?",
        opts=[
            "a model that loops and repeats itself",
            "a model that lacks knowledge of a new private API",
            "a model that must learn a language it never saw",
            "a model trained on no data",
        ],
        answer="A",
        why="WSP removes obstacles; it does not add knowledge.",
    ),
    dict(
        id="core-verify",
        kind="core",
        q=(
            "An edit moves the logit margin by up to 1.87 but the top-1 token never "
            "changes and eval accuracy is identical. What should the editor do next?"
        ),
        opts=[
            "increase alpha until accuracy moves",
            "reject the direction as non-causal and search for a different mechanism",
            "keep the edit, the logit movement is the result",
            "report success",
        ],
        answer="B",
        why="Behaviour is the dependent variable; logit movement alone is not evidence.",
    ),
    dict(
        id="core-role",
        kind="core",
        q=(
            "In the blog's experiment, which model reads the tensors and writes the "
            "edit: the 0.8B model being improved, or a separate frontier model?"
        ),
        opts=[
            "the 0.8B model edits itself",
            "a separate frontier model acts as the programmer",
            "both edit each other",
            "no model is involved",
        ],
        answer="B",
        why="The 0.8B is the patient; GPT-6 Astra is the doctor. This is the crux.",
    ),
]

# The dissociation this bench is built to expose. `recite` items can be answered
# by recalling the textbook claim; `apply` items require making the same
# discrimination about the experiment actually in front of you.
MODE = {
    "core-linalg": "recite",
    "core-read": "recite",
    "core-decode": "recite",
    "core-site": "recite",
    "core-alpha": "recite",
    "core-obstacle": "recite",
    "core-missing": "apply",
    "core-grammar": "apply",
    "core-scope": "apply",
    "core-verify": "apply",
    "core-role": "apply",
}


def build_prompt(tokenizer, item) -> str:
    letters = "ABCD"
    body = item["q"] + "\n\n" + "\n".join(
        f"{letters[i]}) {opt}" for i, opt in enumerate(item["opts"])
    )
    body += "\n\nReply with exactly one line: Answer: <letter>"
    messages = [
        {"role": "system", "content": SYSTEM},
        {"role": "user", "content": body},
    ]
    return tokenizer.apply_chat_template(
        messages, tokenize=False, add_generation_prompt=True, enable_thinking=False
    )


def parse_letter(text: str) -> str | None:
    matches = re.findall(r"(?:Answer:\s*)?\b([ABCD])\b", text)
    return matches[-1] if matches else None


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", default=str(OUT / "model35"))
    parser.add_argument("--dtype", default="bf16", choices=["bf16", "fp32"])
    parser.add_argument("--name", default="doctor")
    parser.add_argument("--max-new", type=int, default=64)
    args = parser.parse_args()

    torch.set_num_threads(8)
    dtype = torch.bfloat16 if args.dtype == "bf16" else torch.float32
    tokenizer = AutoTokenizer.from_pretrained(args.model)
    model = AutoModelForCausalLM.from_pretrained(
        args.model, dtype=dtype, low_cpu_mem_usage=True
    ).eval()

    results = []
    for item in ITEMS:
        text = build_prompt(tokenizer, item)
        encoded = tokenizer(text, return_tensors="pt", truncation=True, max_length=2048)
        with torch.inference_mode():
            generated = model.generate(
                **encoded,
                max_new_tokens=args.max_new,
                do_sample=False,
                pad_token_id=tokenizer.pad_token_id or tokenizer.eos_token_id,
            )
        completion = tokenizer.decode(
            generated[0, encoded["input_ids"].shape[1] :], skip_special_tokens=True
        )
        got = parse_letter(completion)
        ok = got == item["answer"]
        results.append(
            dict(id=item["id"], kind=item["kind"], answer=item["answer"], got=got,
                 ok=ok, raw=completion.strip()[:200])
        )
        print(f"{'PASS' if ok else 'FAIL'} {item['id']:<15} want={item['answer']} got={got} raw={completion.strip()[:60]!r}")

    core = [r for r in results if r["kind"] == "core"]
    ctl = [r for r in results if r["kind"] == "control"]
    summary = {
        "model": args.model,
        "dtype": args.dtype,
        "core_correct": sum(r["ok"] for r in core),
        "core_total": len(core),
        "control_correct": sum(r["ok"] for r in ctl),
        "control_total": len(ctl),
        "results": results,
    }
    print(
        f"\n{args.name}: core {summary['core_correct']}/{summary['core_total']} "
        f"(chance {len(core)/4:.1f}), controls {summary['control_correct']}/{summary['control_total']}"
    )
    (OUT / f"{args.name}.json").write_text(json.dumps(summary, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
