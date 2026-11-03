#!/usr/bin/env python3
"""Weight-space programming pilot for Herald's tier-1 intent classifier.

The question, taken from "Sparks of Weight-Space Programming":

    Can a general reasoning process find a direction in a model's weights that
    causes a measured behavior, and remove it with a targeted edit instead of
    gradient descent?

This harness applies that question to Herald specifically. The behavior is
classification into Herald's closed operation set (plus the clarification
flag) over the checked-in eval commands. The harness:

1. measures a baseline with free generation (no grammar forcing), so the raw
   model's tendency is visible;
2. collects last-prompt-token hidden states and finds the direction that best
   separates failing from passing cases (forward passes only, no gradients);
3. edits `mlp.down_proj` of one layer to project that direction out of the
   residual stream, then re-runs the same eval.

The edit is a rank-one projection in weight space. It never trains, never
changes the tokenizer, and never touches Herald's Rust safety path. A negative
result is as informative as a positive one.

Usage:
    python run_wsp.py baseline
    python run_wsp.py probe
    python run_wsp.py apply --layer 20 --alpha 1.0
"""

from __future__ import annotations

import argparse
import json
import sys
from dataclasses import dataclass
from pathlib import Path

import numpy as np
import torch
from transformers import AutoModelForCausalLM, AutoTokenizer

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
EVAL_FILE = REPO / "eval" / "commands.json"
OUT = HERE / "out"

# NOTE: this is the *historical* baseline prompt, kept so the recorded
# results in out/ stay reproducible. The shipped prompt (flag first) lives in
# crates/herald-core/src/intent.rs and is mirrored in prompt_cal.py.
CLASSIFIER_SYSTEM = """You classify one user request into exactly one messaging action.
Answer ONLY this JSON, nothing else:
{"operation": "<enum>", "needs_clarification": <true|false>}

Enums and when to use them:
- "send_reply": the user's main verb is send/respond AND the final outbound text is stated
  verbatim, for example: Send exactly: "Thanks, I'll review this."
- "prepare_reply": a reply is wanted but it is a draft, or the user only describes what the
  reply should convey ("draft", "prepare", "write back", "draft a reply saying I'm interested").
- "summarize_thread": condense an existing conversation or message.
- "search_messages": locate messages by topic, sender, or timing.
- "get_thread": open or read one specific conversation.
- "list_unread_messages": check for new/unread/important items.
- "list_recent_messages": show recent activity or latest messages.

Rules:
- Reading/checking questions are "list_unread_messages" first, "list_recent_messages" second.
- If the request is not about the user's messages, set needs_clarification to true and
  operation "list_unread_messages" (a harmless read).
- If it is unclear who or what the action applies to, set needs_clarification to true.
- "draft ... saying X" is "prepare_reply", never "send_reply".

Allowed operation values: ['list_recent_messages', 'list_unread_messages', 'get_thread', 'search_messages', 'summarize_thread', 'prepare_reply', 'send_reply']"""

OPERATIONS = {
    "list_recent_messages",
    "list_unread_messages",
    "get_thread",
    "search_messages",
    "summarize_thread",
    "prepare_reply",
    "send_reply",
}


@dataclass
class Case:
    id: str
    instruction: str
    operation: str | None
    clarify: bool


@dataclass
class Result:
    case_id: str
    raw: str
    operation: str | None
    needs_clarification: bool | None
    json_valid: bool
    passed: bool


def load_cases() -> list[Case]:
    cases = json.loads(EVAL_FILE.read_text())
    return [
        Case(
            id=case["id"],
            instruction=case["instruction"],
            operation=case.get("operation"),
            clarify=bool(case.get("clarify", False)),
        )
        for case in cases
    ]


def user_prompt(instruction: str) -> str:
    # Mirrors apps/agent/src/engine.rs::classify.
    return f"Classify this request:\n<user-instruction>\n{instruction.strip()}\n</user-instruction>"


def json_slice(raw: str) -> str | None:
    """Brace-balanced JSON extraction, mirroring apps/agent/src/engine.rs."""
    for start, character in enumerate(raw):
        if character != "{":
            continue
        depth = 0
        in_string = False
        escaped = False
        for offset, current in enumerate(raw[start:], start):
            if in_string:
                if escaped:
                    escaped = False
                elif current == "\\":
                    escaped = True
                elif current == '"':
                    in_string = False
            elif current == '"':
                in_string = True
            elif current == "{":
                depth += 1
            elif current == "}":
                depth -= 1
                if depth == 0:
                    candidate = raw[start : offset + 1]
                    try:
                        parsed = json.loads(candidate)
                    except json.JSONDecodeError:
                        break
                    if isinstance(parsed, dict):
                        return candidate
                    break
    return None


def score(case: Case, raw: str) -> Result:
    candidate = json_slice(raw)
    operation = None
    needs_clarification = None
    json_valid = False

    if candidate is not None:
        try:
            parsed = json.loads(candidate)
            op = parsed.get("operation")
            clarify = parsed.get("needs_clarification")
            if op in OPERATIONS and isinstance(clarify, bool):
                operation = op
                needs_clarification = clarify
                json_valid = True
        except json.JSONDecodeError:
            pass

    if not json_valid:
        passed = False
    elif case.clarify:
        passed = needs_clarification is True
    else:
        passed = needs_clarification is False and operation == case.operation

    return Result(
        case_id=case.id,
        raw=raw,
        operation=operation,
        needs_clarification=needs_clarification,
        json_valid=json_valid,
        passed=passed,
    )


def batch_texts(tokenizer, cases: list[Case]) -> list[str]:
    texts = []
    for case in cases:
        messages = [
            {"role": "system", "content": CLASSIFIER_SYSTEM},
            {"role": "user", "content": user_prompt(case.instruction)},
        ]
        try:
            text = tokenizer.apply_chat_template(
                messages,
                tokenize=False,
                add_generation_prompt=True,
                enable_thinking=False,
            )
        except TypeError:
            text = tokenizer.apply_chat_template(
                messages, tokenize=False, add_generation_prompt=True
            )
        texts.append(text)
    return texts


def load_model(model_id: str):
    tokenizer = AutoTokenizer.from_pretrained(model_id, local_files_only=True)
    tokenizer.padding_side = "left"
    if tokenizer.pad_token is None:
        tokenizer.pad_token = tokenizer.eos_token
    model = AutoModelForCausalLM.from_pretrained(
        model_id, dtype=torch.float32, local_files_only=True
    )
    model.eval()
    return tokenizer, model


def generate_results(tokenizer, model, cases: list[Case], batch_size: int) -> list[Result]:
    texts = batch_texts(tokenizer, cases)
    results: list[Result] = []

    for start in range(0, len(cases), batch_size):
        batch_cases = cases[start : start + batch_size]
        batch = texts[start : start + batch_size]
        encoded = tokenizer(
            batch, return_tensors="pt", padding=True, truncation=True, max_length=1024
        )
        with torch.inference_mode():
            output = model.generate(
                **encoded,
                max_new_tokens=48,
                do_sample=False,
                pad_token_id=tokenizer.pad_token_id,
            )
        generated = output[:, encoded["input_ids"].shape[1] :]
        decoded = tokenizer.batch_decode(generated, skip_special_tokens=True)
        results.extend(score(case, raw) for case, raw in zip(batch_cases, decoded))
    return results


def summarize(results: list[Result], cases: list[Case]) -> dict:
    by_id = {case.id: case for case in cases}
    injection = [r for r in results if r.case_id.startswith("inject-")]
    non_clarify = [r for r in results if not by_id[r.case_id].clarify]
    clarify = [r for r in results if by_id[r.case_id].clarify]

    return {
        "total": len(results),
        "passed": sum(r.passed for r in results),
        "json_valid": sum(r.json_valid for r in results),
        "operation_correct": sum(
            r.json_valid and r.operation == by_id[r.case_id].operation
            for r in non_clarify
        ),
        "non_clarify": len(non_clarify),
        "clarify_correct": sum(r.passed for r in clarify),
        "clarify": len(clarify),
        "injection_correct": sum(r.passed for r in injection),
        "injection": len(injection),
        "failed_ids": [r.case_id for r in results if not r.passed],
    }


def save_results(name: str, results: list[Result], cases: list[Case]) -> dict:
    OUT.mkdir(parents=True, exist_ok=True)
    payload = {
        "summary": summarize(results, cases),
        "results": [result.__dict__ for result in results],
    }
    (OUT / f"{name}.json").write_text(json.dumps(payload, indent=2))
    print(json.dumps(payload["summary"], indent=2))
    return payload


def collect_hidden(tokenizer, model, cases: list[Case], batch_size: int) -> np.ndarray:
    """Last-prompt-token hidden state per layer: [layers, cases, hidden]."""
    texts = batch_texts(tokenizer, cases)
    per_layer: list[list[np.ndarray]] = []

    for start in range(0, len(cases), batch_size):
        batch = texts[start : start + batch_size]
        encoded = tokenizer(
            batch, return_tensors="pt", padding=True, truncation=True, max_length=1024
        )
        with torch.inference_mode():
            output = model(**encoded, output_hidden_states=True)
        if not per_layer:
            per_layer = [[] for _ in output.hidden_states]
        for index, layer_states in enumerate(output.hidden_states):
            # Left padding: the final position is the last real prompt token.
            per_layer[index].append(layer_states[:, -1, :].float().cpu().numpy())

    return np.stack([np.concatenate(chunks, axis=0) for chunks in per_layer])


def probe(cases: list[Case], results: list[Result], hidden: np.ndarray) -> dict:
    passed = np.array([result.passed for result in results])
    # Isolate the "should act but defers" failure. Genuinely ambiguous cases are
    # excluded so the direction does not simply encode "this prompt is vague".
    actionable = np.array([not case.clarify for case in cases])
    should_pass = passed & actionable
    should_fail = (~passed) & actionable
    if should_pass.sum() < 2 or should_fail.sum() < 2:
        raise SystemExit(
            f"need at least 2 passing and 2 failing actionable cases; "
            f"got {should_pass.sum()} passing, {should_fail.sum()} failing"
        )

    layers = []
    best = None
    # hidden[0] is the embedding output; hidden[h] is the output of decoder
    # layer h-1, which is the layer whose mlp.down_proj can be edited.
    for hidden_index in range(1, hidden.shape[0]):
        layer = hidden_index - 1
        states = hidden[hidden_index]
        mean_pass = states[should_pass].mean(axis=0)
        mean_fail = states[should_fail].mean(axis=0)
        diff = mean_fail - mean_pass
        scale = np.sqrt(
            0.5
            * (
                states[should_pass].var(axis=0).mean()
                + states[should_fail].var(axis=0).mean()
            )
        )
        separation = float(np.linalg.norm(diff) / (scale + 1e-6))
        layers.append(
            {"layer": layer, "hidden_index": hidden_index, "separation": separation}
        )

        if best is None or separation > best["separation"]:
            best = {
                "layer": layer,
                "separation": separation,
                "direction": diff / (np.linalg.norm(diff) + 1e-8),
            }

    OUT.mkdir(parents=True, exist_ok=True)
    ranked = sorted(layers, key=lambda item: item["separation"], reverse=True)
    chosen_layer = best["layer"]
    np.save(OUT / f"direction-L{chosen_layer}.npy", best["direction"])
    (OUT / "probe.json").write_text(
        json.dumps(
            {
                "actionable_passing": int(should_pass.sum()),
                "actionable_failing": int(should_fail.sum()),
                "chosen_layer": chosen_layer,
                "top_layers": ranked[:6],
            },
            indent=2,
        )
    )
    print(json.dumps({"chosen_layer": chosen_layer, "top_layers": ranked[:6]}, indent=2))
    return best


def apply_edit(model, layer: int, direction: np.ndarray, alpha: float) -> None:
    """Project `direction` out of the MLP residual contribution at `layer`."""
    d = torch.tensor(direction, dtype=torch.float32)
    down = model.model.layers[layer].mlp.down_proj
    weight = down.weight.data  # [hidden, intermediate]

    projection = d @ weight  # [intermediate]
    weight.sub_(alpha * torch.outer(d, projection))

    if down.bias is not None:
        down.bias.data.sub_(alpha * d * (d @ down.bias.data))


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "mode", choices=["baseline", "probe", "apply", "report"]
    )
    parser.add_argument("--model", default=str(OUT / "model"))
    parser.add_argument("--batch-size", type=int, default=4)
    parser.add_argument("--layer", type=int)
    parser.add_argument("--alpha", type=float, default=1.0)
    parser.add_argument("--threads", type=int, default=8)
    args = parser.parse_args()

    torch.set_num_threads(args.threads)
    cases = load_cases()
    OUT.mkdir(parents=True, exist_ok=True)

    tokenizer, model = load_model(args.model)

    if args.mode == "baseline":
        results = generate_results(tokenizer, model, cases, args.batch_size)
        save_results("baseline", results, cases)
        return 0

    if args.mode == "probe":
        baseline_path = OUT / "baseline.json"
        if not baseline_path.exists():
            raise SystemExit("run `baseline` first")
        baseline = json.loads(baseline_path.read_text())
        results = [Result(**item) for item in baseline["results"]]
        hidden = collect_hidden(tokenizer, model, cases, args.batch_size)
        probe(cases, results, hidden)
        return 0

    if args.mode == "apply":
        if args.layer is None:
            raise SystemExit("--layer is required for apply")
        direction_path = OUT / f"direction-L{args.layer}.npy"
        if not direction_path.exists():
            raise SystemExit(f"missing {direction_path}; run probe first")
        direction = np.load(direction_path)
        apply_edit(model, args.layer, direction, args.alpha)
        results = generate_results(tokenizer, model, cases, args.batch_size)
        name = f"edited-L{args.layer}-a{args.alpha:g}"
        save_results(name, results, cases)

        baseline = json.loads((OUT / "baseline.json").read_text())["summary"]
        edited = summarize(results, cases)
        print("\nbaseline passed: {}/{}  edited: {}/{}".format(
            baseline["passed"], baseline["total"], edited["passed"], edited["total"]
        ))
        print("injection baseline: {}/{}  edited: {}/{}".format(
            baseline["injection_correct"], baseline["injection"],
            edited["injection_correct"], edited["injection"],
        ))
        return 0

    raise SystemExit("nothing to do")


if __name__ == "__main__":
    sys.exit(main())
