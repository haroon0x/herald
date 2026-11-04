# Models

`manifest.json` pins the default local planner model. `herald init` copies it
into the data directory, and `herald model download` fetches the exact artifact,
checks its SHA-256, and refuses anything else.

| Field | Value |
| --- | --- |
| Model | Qwen3.5-0.8B Instruct |
| Quantization | Q4_K_M |
| File | `Qwen3.5-0.8B-Q4_K_M.gguf` |
| Size | 532,517,120 bytes (about 508 MB) |
| Source | [`unsloth/Qwen3.5-0.8B-GGUF`](https://huggingface.co/unsloth/Qwen3.5-0.8B-GGUF) |
| Revision | `6ab461498e2023f6e3c1baea90a8f0fe38ab64d0` |
| SHA-256 | `bd258782e35f7f458f8aced1adc053e6e92e89bc735ba3be89d38a06121dc517` |
| License | Apache-2.0 |

The model is optional. With `[model] backend = "none"` Herald runs on the
deterministic rules router alone and never starts a process.

```bash
herald init                     # writes models/manifest.json into the data dir
herald model download           # resumable download + SHA-256 verification
herald model status             # backend, manifest, file, digest
herald model verify             # re-hash the file on disk
```

If you already have a GGUF, skip the download and pin its digest yourself:

```bash
herald model pin ./Qwen3.5-0.8B-Q4_K_M.gguf \
  --name "Qwen3.5-0.8B Instruct" \
  --repo unsloth/Qwen3.5-0.8B-GGUF \
  --revision 6ab461498e2023f6e3c1baea90a8f0fe38ab64d0 \
  --quantization Q4_K_M
```

`model pin` hashes the file and writes a manifest whose `filename` is an
absolute path. It does not copy the model.

The manifest is data, not code. Changing it changes which artifact is trusted;
review such a change like a dependency update. A placeholder digest is refused
by both `download` and the managed runtime.
