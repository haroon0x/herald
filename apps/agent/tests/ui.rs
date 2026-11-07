//! End-to-end test for `herald ui`.
//!
//! Spawns the real loopback server, checks that a request without the
//! per-run token is refused, that the token'd page and API calls succeed
//! with the expected headers, and that the UI can queue an approval without
//! ever sending it.

use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use assert_cmd::prelude::*;
use tempfile::TempDir;

struct Server {
    child: Child,
    port: u16,
    token: String,
}

impl Server {
    fn get(&self, path: &str) -> HttpResponse {
        request(
            self.port,
            "GET",
            path,
            &[("X-Herald-Token", &self.token)],
            None,
        )
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // Kill and reap the server even when an assertion panicked, so a
        // failing test never leaves a `herald ui` process behind.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct HttpResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(header_name, _)| header_name == name)
        .map(|(_, value)| value.as_str())
}

fn request(
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> HttpResponse {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to the UI server");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .expect("write timeout");

    let mut encoded =
        format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n");
    for (name, value) in headers {
        let _ = write!(encoded, "{name}: {value}\r\n");
    }
    if let Some(payload) = body {
        let _ = write!(encoded, "Content-Length: {}\r\n", payload.len());
    }
    encoded.push_str("\r\n");
    if let Some(payload) = body {
        encoded.push_str(payload);
    }
    stream
        .write_all(encoded.as_bytes())
        .expect("write the HTTP request");

    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader
        .read_line(&mut status_line)
        .expect("read the status line");
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .expect("status code parses");

    let mut headers = Vec::new();
    let mut content_length = None;
    loop {
        let mut line = String::new();
        let count = reader.read_line(&mut line).expect("read a header line");
        if count == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_lowercase();
            let value = value.trim().to_string();
            if name == "content-length" {
                content_length = value.parse().ok();
            }
            headers.push((name, value));
        }
    }

    let mut body_bytes = Vec::new();
    if let Some(length) = content_length {
        let mut buffer = vec![0_u8; length];
        reader
            .read_exact(&mut buffer)
            .expect("read the Content-Length body");
        body_bytes = buffer;
    } else {
        reader
            .read_to_end(&mut body_bytes)
            .expect("read the body until close");
    }

    HttpResponse {
        status,
        headers,
        body: String::from_utf8_lossy(&body_bytes).into_owned(),
    }
}

/// Extract the bound port and per-run token from the startup line, printed
/// as `Herald UI  http://127.0.0.1:<port>/?token=<token>`.
fn parse_startup_url(line: &str) -> (u16, String) {
    let url = line
        .split_whitespace()
        .find(|word| word.starts_with("http://"))
        .expect("startup line contains the URL");
    let authority = url
        .strip_prefix("http://127.0.0.1:")
        .expect("URL is loopback");
    let port: u16 = authority
        .split('/')
        .next()
        .and_then(|digits| digits.parse().ok())
        .expect("port parses from the startup URL");
    let token = url
        .split("token=")
        .nth(1)
        .map_or("", |rest| rest.split('&').next().unwrap_or(""))
        .trim()
        .to_string();
    assert!(
        !token.is_empty(),
        "token parses from the startup URL: {line}"
    );
    (port, token)
}

fn spawn_server(data: &Path, config: &Path) -> Server {
    let mut child = Command::cargo_bin("herald")
        .expect("binary builds")
        .arg("--data-dir")
        .arg(data)
        .arg("--config")
        .arg(config)
        .arg("ui")
        .arg("--port")
        .arg("0")
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn herald ui");

    let stdout = child.stdout.take().expect("stdout is piped");
    // Read the startup line on a background thread so a missing or unflushed
    // line can never hang the test: the bounded recv decides the failure.
    let (sender, receiver) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let _ = reader.read_line(&mut line);
        let _ = sender.send(line);
        // Keep draining until the child exits so it never writes to a closed
        // pipe and dies of EPIPE mid-startup.
        let mut rest = String::new();
        let _ = reader.read_to_string(&mut rest);
    });

    let first_line = receiver
        .recv_timeout(Duration::from_secs(15))
        .expect("startup line within 15 seconds");
    let (port, token) = parse_startup_url(&first_line);
    Server { child, port, token }
}

#[test]
fn ui_ask_queues_an_approval_and_sends_nothing() {
    let dir: TempDir = tempfile::tempdir().expect("tempdir");
    let data: PathBuf = dir.path().join("data");
    let config = dir.path().join("config.toml");
    std::fs::create_dir_all(&data).expect("data dir");

    let init_output = Command::cargo_bin("herald")
        .expect("binary builds")
        .arg("--data-dir")
        .arg(&data)
        .arg("--config")
        .arg(&config)
        .arg("init")
        .output()
        .expect("init runs");
    assert!(init_output.status.success(), "init failed");

    let server = spawn_server(&data, &config);
    let port = server.port;
    let token = server.token.clone();

    let body = "herald e2e: ask should queue, never send";
    let queued = request(
        port,
        "POST",
        "/api/ask",
        &[
            ("X-Herald-Token", token.as_str()),
            ("Content-Type", "application/json"),
        ],
        Some(&format!(
            "{{\"instruction\":\"Send exactly: '{body}' to the founder\",\"provider\":\"fake\",\"enable_send\":true}}"
        )),
    );
    assert_eq!(queued.status, 200, "ask body: {}", queued.body);
    assert!(
        queued.body.contains("\"awaiting_approval\""),
        "ask response: {}",
        queued.body
    );
    assert!(
        queued.body.contains(body),
        "queue response echoes the exact body: {}",
        queued.body
    );

    let approvals = server.get("/api/approvals");
    assert!(
        approvals.body.contains("\"count\":1"),
        "approvals: {}",
        approvals.body
    );
    assert!(
        approvals.body.contains(body),
        "approval carries the exact body: {}",
        approvals.body
    );

    // No approval has been executed: the action is still waiting.
    let history = server.get("/api/history?limit=5");
    assert!(
        history.body.contains("\"awaiting_approval\""),
        "history shows the pending approval, nothing sent: {}",
        history.body
    );
}

#[test]
fn ui_server_requires_the_token_and_serves_authenticated_requests() {
    let dir: TempDir = tempfile::tempdir().expect("tempdir");
    let data: PathBuf = dir.path().join("data");
    let config = dir.path().join("config.toml");
    std::fs::create_dir_all(&data).expect("data dir");

    let init_output = Command::cargo_bin("herald")
        .expect("binary builds")
        .arg("--data-dir")
        .arg(&data)
        .arg("--config")
        .arg(&config)
        .arg("init")
        .output()
        .expect("init runs");
    assert!(
        init_output.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&init_output.stderr)
    );

    let server = spawn_server(&data, &config);
    let port = server.port;
    let token = server.token.clone();

    // The startup line is printed after the URL file is written, so by the
    // time we have the token the file must exist and carry it.
    let url_file = data.join("ui-url");
    let url_contents = std::fs::read_to_string(&url_file).expect("ui-url file is written");
    assert!(
        url_contents.contains(&format!("http://127.0.0.1:{port}/?token={token}")),
        "ui-url contains the printed URL with the per-run token"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&url_file)
            .expect("ui-url metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "ui-url must be mode 0600");
    }

    // No token: the page is refused.
    let denied = request(port, "GET", "/", &[], None);
    assert_eq!(denied.status, 403, "GET / without a token must be 403");

    // Token in the URL: the page renders with the token substituted and a
    // strict CSP.
    let page = request(port, "GET", &format!("/?token={token}"), &[], None);
    assert_eq!(page.status, 200);
    assert!(page.body.contains("Herald"), "page names Herald");
    assert!(
        !page.body.contains("__TOKEN__"),
        "token slot must be substituted"
    );
    let csp = header(&page.headers, "content-security-policy").expect("CSP header present");
    assert!(csp.contains("default-src 'self'"), "CSP is strict: {csp}");

    // Authenticated API calls succeed.
    let meta = server.get("/api/meta");
    assert_eq!(meta.status, 200);
    assert!(
        meta.body.contains("\"ok\":true"),
        "meta body: {}",
        meta.body
    );

    let history = server.get("/api/history?limit=3");
    assert_eq!(history.status, 200);
    assert!(
        history.body.contains("\"ok\":true"),
        "history body: {}",
        history.body
    );

    // A wrong token is rejected.
    let wrong = request(
        port,
        "GET",
        "/api/meta",
        &[("X-Herald-Token", "wrong")],
        None,
    );
    assert!(
        wrong.body.contains("\"ok\":false"),
        "wrong-token body: {}",
        wrong.body
    );

    // The removed posting endpoints are unknown to the router.
    for path in ["/api/post", "/api/autopost"] {
        let gone = request(
            port,
            "POST",
            path,
            &[
                ("X-Herald-Token", &token),
                ("Content-Type", "application/json"),
            ],
            Some("{}"),
        );
        assert!(
            gone.body.contains("\"ok\":false"),
            "{path} must be unknown: {}",
            gone.body
        );
    }

    // Nothing wrote a kill-switch file; the lane no longer exists.
    assert!(!data.join("autopost.disabled").exists());
}
