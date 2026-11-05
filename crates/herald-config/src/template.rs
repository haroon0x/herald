//! The starter `config.toml` written by `herald init`. Minimal and safe by default.

/// Annotated starter configuration written by `herald init`.
pub const TEMPLATE: &str = r#"# Herald configuration
#
# Precedence: command-line flags > environment variables > this file > defaults.
# Docs: https://github.com/haroon0x/herald/blob/master/docs/configuration.md

# Provider used when --provider is not passed. `fake` is the safe demo inbox.
# Use `maildir` after filling in the [maildir] section below.
default_provider = "fake"

[policy]
# Sending is disabled by default. Enabling it only allows an approval request;
# Herald still never sends without `herald approve <id>`.
allow_send = false
max_body_chars = 8000
# How long a pending approval keeps its exact body before it expires.
approval_ttl_hours = 72
# Autopost lane: ORIGINAL X posts only. Never DMs, never replies.
# Off by default. `herald autopost off` is the kill switch.
# allow_autopost = false
# Posts per calendar day (UTC), counting manual and autonomous posts.
# autopost_daily_limit = 5
# Deterministic quality floor in [0, 10].
# autopost_min_quality = 7
# UTC quiet window where autopost is blocked, "HH:MM-HH:MM" (wrap-around
# allowed, e.g. "22:00-06:00"). Omit to disable.
# autopost_quiet_hours = "22:00-06:00"

[model]
# none: rules-only (no process, no model). managed: pinned llama-server.
# openai: any OpenAI-compatible local endpoint (Ollama, LM Studio, vLLM).
# opencode: local `opencode run --standalone --model <id>`, one-shot.
# A managed GGUF, when present, is only a lazy fallback for provider failures.
backend = "none"
# For opencode, model_name is provider/model, optionally with #variant.
# model_name = "opencode-go/deepseek-v4.1-flash#low"
# endpoint = "http://127.0.0.1:11434/v1"
# model_name = "qwen2.5:1.5b-instruct"
# manifest = "manifest.json"
# api_key_env = "HERALD_MODEL_API_KEY"
context_size = 4096
threads = 4

# Local email. Uncomment and edit to read a real mailbox.
# [maildir]
# path = "~/Mail"
# sent_path = "~/.Sent"
# from = "Your Name <you@example.com>"
#
# # Optional. Without SMTP the mailbox is read-only and sending is refused.
# [maildir.smtp]
# host = "smtp.example.com"
# port = 587
# username = "you@example.com"
# # Prefer the environment over a plaintext password in this file:
# #   export HERALD_SMTP_PASSWORD=...
# password_env = "HERALD_SMTP_PASSWORD"
# security = "starttls"   # starttls | tls | none

# X (Twitter) browser adapter. Uncomment and edit to use it.
# The browser profile is dedicated: never point profile_dir at your
# everyday Chrome profile. Login is done once by the user in this
# profile; Herald never extracts cookies or passwords.
# [x]
# profile_dir = "~/.local/share/herald/profiles/x"  # default under data dir
# handle = "@you"  # display-only handle
# headless = true
# action_delay_ms = 1800
# typing_delay_ms = 20
# close_after = true
# read_only = true refuses every write while keeping reads working.
"#;
