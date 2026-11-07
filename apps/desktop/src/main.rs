//! The Herald desktop shell.
//!
//! It is a thin wrapper, not a second Herald: on startup it spawns
//! `herald ui --port 0`, reads the token-carrying URL from the child's first
//! stdout line, and opens that URL in a WebView. The page, the loopback server,
//! the approval queue, and every safety property are the CLI's own — this shell
//! adds no commands, no IPC, and no second execution path (see ADR 0010 and
//! ADR 0011: the shell wraps the UI; it does not reimplement it).
//!
//! The `herald` binary is found by, in order: `$HERALD_BIN`, a sibling of this
//! executable, or `herald` on `PATH`.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

/// The spawned `herald ui` child. Killed on drop, so closing the window stops
/// the server too.
struct ServerChild(Child);

impl Drop for ServerChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn herald_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("HERALD_BIN") {
        return PathBuf::from(path);
    }
    if let Ok(executable) = std::env::current_exe() {
        if let Some(directory) = executable.parent() {
            let sibling = directory.join("herald");
            if sibling.exists() {
                return sibling;
            }
        }
    }
    PathBuf::from("herald")
}

/// The URL token'd page printed by `herald ui` on its first stdout line:
/// `Herald UI  http://127.0.0.1:<port>/?token=<token>`.
fn parse_startup_url(line: &str) -> Option<String> {
    line.split_whitespace()
        .find(|word| word.starts_with("http://127.0.0.1:"))
        .map(str::to_string)
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let binary = herald_binary();
            let mut child = Command::new(&binary)
                .args(["ui", "--port", "0"])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|error| format!("could not spawn {}: {error}", binary.display()))?;
            let stdout = child.stdout.take().expect("stdout is piped");
            let mut reader = BufReader::new(stdout);
            let mut first_line = String::new();
            reader
                .read_line(&mut first_line)
                .map_err(|error| format!("could not read herald ui startup: {error}"))?;
            let url = parse_startup_url(&first_line)
                .ok_or("herald ui did not print its URL".to_string())?;
            app.manage(ServerChild(child));
            WebviewWindowBuilder::new(
                app,
                "main",
                WebviewUrl::External(
                    url.parse::<tauri::Url>()
                        .map_err(|error| error.to_string())?,
                ),
            )
            .title("Herald")
            .inner_size(1200.0, 800.0)
            .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to run the Herald desktop shell");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_url_is_found_in_the_first_line() {
        assert_eq!(
            parse_startup_url("Herald UI  http://127.0.0.1:7878/?token=abc123"),
            Some("http://127.0.0.1:7878/?token=abc123".to_string())
        );
        assert_eq!(parse_startup_url("nothing here"), None);
    }
}
