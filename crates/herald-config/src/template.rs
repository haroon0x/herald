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

[model]
# none: rules-only (no process, no model). managed: pinned llama-server.
# openai: any OpenAI-compatible local endpoint (Ollama, LM Studio, vLLM).
backend = "none"
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
"#;
