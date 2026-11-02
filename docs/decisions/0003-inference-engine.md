# 0003: Inference engine — llama.cpp sidecar (not vLLM)

Date: 2026-11-02
Status: Accepted

## Decision

`llama.cpp` (`llama-server`) is the pinned local inference runtime. The planner
stays rules-first with the model as a fallback, so inference is the rare path.

## Why not vLLM

vLLM is a strong engine, but it is optimized for exactly the workload Herald
does not have:

- **Concurrency.** vLLM's PagedAttention + continuous batching pay off at 4-8+
  concurrent streams (44x more throughput at 64 clients in Red Hat's 2026
  benchmark). Herald serves one user; batch=1 throughput is comparable between
  engines, so vLLM's advantage never materializes while its costs always do.
- **Install surface.** HIPAA of the product constraints: the user installs
  nothing. vLLM requires pip, a CUDA stack, 45-90s startup, and on Windows it
  is WSL-only. llama.cpp ships a single native binary and one GGUF file.
- **Memory.** vLLM preallocates a KV pool (blocking ~0.9 GB on small cards);
  llama.cpp allocates by context and keeps host RAM overhead near 1.8 GB vs
  vLLM's ~4.6 GB. On consumer laptops that headroom buys model size, not
  scheduler machinery.
- **Fit.** Ollama and LM Studio are llama.cpp underneath; anchoring on it is a
  bet for the ecosystem.

## When this decision is revisited

- If Herald ever ships a hosted multi-user layer, that is a server-side product
  and would use vLLM/SGLang — that decision does not belong to this repository.
- `mistral.rs` (Rust-native, GGUF) remains the documented alternative; the
  runtime interface in `herald-runtime` stays abstract so a swap is contained.

## Notes from the spike checklist

- Sub-2B models on discrete GPUs gain little below Q8_0; Q4_K_M/Q6_K are for
  CPU decode (bandwidth-bound), which is Herald's default target.
- llama.cpp thread defaults over-subscribe on hybrid CPUs; plan a knob, not
  folklore (`--threads` equals physical P-cores is a starting point).
- Long-prompt prefill is llama.cpp's weaker stage; Herald's planner prompts are
  short by policy (no full inboxes to the model), which keeps this irrelevant.
