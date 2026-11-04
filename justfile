# Herald developer tasks. Requires just; no plugins.

# List the available recipes.
default:
    @just --list

# Format the workspace.
fmt:
    cargo fmt --all

# Verify formatting without writing.
fmt-check:
    cargo fmt --all --check

# Lint every target and deny warnings.
clippy:
    cargo clippy --workspace --all-targets --locked -- -D warnings

# Run the full test suite.
test:
    cargo test --workspace --all-targets --locked

# Build API documentation and deny rustdoc warnings.
doc:
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked

# Run every gate CI runs.
check: fmt-check clippy test doc

# Check RustSec advisories (requires `cargo install cargo-audit`).
audit:
    cargo audit

# Check licenses, bans, advisories, and sources (requires `cargo install cargo-deny`).
deny:
    cargo deny check

# Run the herald CLI: `just run doctor`, `just run ask "..."`.
run *args:
    cargo run -p herald-agent --bin herald -- {{args}}

# Score the checked-in command set: `just eval --engine rules`.
eval *args:
    cargo run -p herald-agent --bin herald -- eval {{args}}

# Build optimized binaries for the host.
release-build:
    cargo build --workspace --release --locked
