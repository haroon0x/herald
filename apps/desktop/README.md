# Herald desktop shell (Tauri v2)

A thin wrapper around `herald ui`: on startup it spawns the `herald` binary with
`ui --port 0`, reads the token-carrying URL from the child's first stdout line,
and opens that URL in a WebView. The page, loopback server, approval queue, and
the CLI's safety properties are unchanged — the shell adds no commands, no IPC,
and no second execution path. See [ADR 0010](../../docs/decisions/0010-local-web-ui.md).

This crate is its own Cargo workspace on purpose: it links WebKitGTK on Linux,
which the herald workspace's CI does not install.

## Build prerequisites (Linux)

```bash
sudo apt install libwebkit2gtk-4.1-dev librsvg2-dev patchelf
```

No tray is used, so no appindicator package is needed. Verified building on
Ubuntu 24.04 with rustc 1.99.

## Run (development)

```bash
# in the herald repo root, build the CLI first so the shell can find it
cargo build -p herald-agent

# then, from this directory
HERALD_BIN=../target/debug/herald cargo run
```

Without `HERALD_BIN`, the shell looks for a `herald` binary next to its own
executable, then on `PATH` (e.g. after `cargo install --path ../agent`).

## Notes

- **Not in the herald workspace.** Its dependency graph (Tauri, WebKit) is
  deliberately isolated; `cargo build` / `cargo test` in the repo root never
  compiles it.
- **Release bundling works.** From the repo root:

  ```bash
  cargo build -p herald-agent --release
  cp target/release/herald apps/desktop/binaries/herald-x86_64-unknown-linux-gnu
  ```

  then from this directory:

  ```bash
  cargo tauri build   # needs `cargo install tauri-cli`
  ```

  producing `target/release/bundle/deb/Herald_0.1.0_amd64.deb` and
  `target/release/bundle/appimage/Herald_0.1.0_amd64.AppImage`. The `herald`
  binary is a Tauri sidecar (`bundle.externalBin`), so the shell finds it next
  to its own executable in the bundle. `binaries/` is gitignored; the sidecar
  file name must carry the target triple. The bundle icon set includes PNG,
  Windows `.ico`, and macOS `.icns` formats; the configured bundle targets
  remain Linux-only in this build.
- **Migrating to a native UI later** is per screen: each page behavior already
  maps to one `herald --json` command, so a screen becomes one IPC command
  behind the same approval queue. See ADR 0010.
