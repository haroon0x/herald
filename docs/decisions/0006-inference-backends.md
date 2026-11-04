# 0006: Inference backends — managed sidecar by default

Date: 2026-11-04
Status: Accepted

## Context

Herald needs a model for two small jobs: classify an instruction into one of
seven operations, and draft a short reply. It does not need a general agent
runtime, and it must not require the user to install Python, CUDA, or a model
server.

The privacy promise is part of the product: message content does not leave the
machine unless the user configures an endpoint that is not local.

## Decision

`herald-runtime` exposes one inference interface with two operations,
`complete_json` and `complete_text`. Two backends implement it:

1. **Managed `llama-server` sidecar (default recommendation).** Herald starts a
   pinned `llama-server` child process with a pinned GGUF, binds it to
   `127.0.0.1`, generates a random API token per run, and shuts it down on drop.
   JSON calls are grammar-constrained with a JSON schema. Generation is
   temperature 0.
2. **OpenAI-compatible endpoint.** `[model] backend = "openai"` points at any
   server that speaks `/v1/chat/completions`: Ollama, LM Studio, vLLM, or a
   remote service. This is the escape hatch for users who already run a model.

`backend = "none"` is the default on a fresh install. The rules router handles
the common commands without inference, and drafting falls back to a local
template.

The managed sidecar is the recommendation. The OpenAI-compatible backend exists
because forcing a bundled binary on users who already have a server is
user-hostile, and because it makes the model tier testable against a mock HTTP
server.

## Why not a cloud API by default

- **Content.** A classification prompt contains the user's instruction, and a
  drafting prompt contains thread context. Sending that to a third party
  contradicts the local-first claim.
- **Availability.** A network dependency in the planning path makes offline use
  impossible and turns an outage into a product outage.
- **Cost and rate limits.** A per-command API bill changes the economics of a
  tool that is meant to be used casually.
- **Trust.** A local binary the user can inspect and a model file with a pinned
  digest are easier to reason about than a vendor endpoint.

A remote endpoint is still possible. It requires an explicit backend and URL,
and `doctor` reports when the endpoint is not loopback. That is the line: the
user can choose to send content off the machine, but Herald will not choose it
for them.

## Why not Ollama as the default

Ollama is a fine product and Herald works with it through the OpenAI-compatible
backend. It is not the default because:

- It is a second installation step and a second version to support.
- Its model naming and lifecycle are its own; a pinned GGUF in a manifest is
  reproducible in a way a tag is not.
- It is a background service. Herald's sidecar lives and dies with the command.

## Why not embed a Rust inference runtime

`mistral.rs` and `candle` are viable and stay documented alternatives. The
reasons not to start there:

- The GGUF ecosystem, quantization support, and grammar-constrained decoding
  are most mature in `llama.cpp`.
- A Rust-native runtime is another large dependency in the trusted build, and
  the runtime interface already isolates the choice.
- The sidecar is a separate process, which limits the blast radius of a parser
  bug in the model loader.

If the runtime interface holds, swapping the backend is a contained change. That
is the point of the trait.

## Grammar-constrained output

The classifier is constrained to a two-field JSON object: a boolean and an enum
of seven operations. The schema is the source of truth, and a test asserts the
prompt and the schema ask for the same field order. Without the constraint, a
0.8B model invents keys and operations often enough to be unusable. With it,
the output is parseable by construction, and the remaining failure mode is a
wrong label rather than a malformed plan.

The drafting path is plain text with no JSON mode. A draft is not a structured
artifact, and forcing JSON there made the output worse.

## Consequences

- The managed backend needs a `llama-server` binary and a GGUF on disk.
  `HERALD_LLAMA_SERVER` points at the binary; the manifest pins the model.
- The sidecar binds to loopback only. There is no bind-address field, so a
  configuration mistake cannot expose it to the network.
- Startup has a 90-second deadline and a health check. A failed start is a clear
  error, not a hang.
- A remote OpenAI-compatible endpoint is a privacy decision the user makes
  explicitly. `doctor` says so.
- The model tier is testable without a real model by pointing the client at a
  mock server.

## Alternatives considered

| Alternative | Why not |
| --- | --- |
| Cloud API as the default | Contradicts local-first and makes offline use impossible. |
| Ollama as the default | Extra install and a service; tags are less reproducible than a pinned digest. |
| vLLM | Optimized for concurrency Herald does not have; heavier install. See [0003](0003-inference-engine.md). |
| Embedded `mistral.rs` | Kept as a documented alternative; the ecosystem is less mature for this workload. |
| No model at all | Rules cover common commands, but drafting and unknown phrasing need a model. The model stays optional. |

## Related

- [0003](0003-inference-engine.md): why `llama.cpp` and not vLLM.
- [../configuration.md](../configuration.md): the `[model]` keys.
- [../architecture.md](../architecture.md): the planner tiers.
