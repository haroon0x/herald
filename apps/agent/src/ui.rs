//! `herald ui` — a local browser UI over the same CLI commands.
//!
//! The server binds to loopback only, requires a per-run token on every API
//! call, rejects cross-origin requests, and never sends anything itself: it
//! shells out to this same binary with `--json`, so the approval queue and the
//! autopost rails are exactly the ones the terminal uses. There is no second
//! execution path.
//!
//! This is also the frontend a desktop shell (Tauri) can wrap unchanged; the
//! page is plain HTML/CSS/JS served from this binary.

use std::io::Read as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use herald_config::{ensure_private_dir, resolve_data_dir};
use serde_json::{Value, json};
use tiny_http::{Header, Method, Request, Response, Server};

use crate::cli::Cli;
use crate::error::AppError;

const MAX_BODY_BYTES: u64 = 64 * 1024;
const INDEX_HTML: &str = include_str!("ui/index.html");
const UI_CSS: &str = include_str!("ui/ui.css");
const UI_JS: &str = include_str!("ui/ui.js");

#[derive(Clone)]
struct Context {
    data_dir: Option<PathBuf>,
    config: Option<PathBuf>,
    token: String,
    port: u16,
    /// One CLI at a time: two X browsers on one profile fight over the lock.
    gate: Arc<Mutex<()>>,
}

/// Start the server and block until the process is stopped.
pub fn serve(cli: &Cli, port: u16, open: bool) -> Result<u8, AppError> {
    let server = Server::http(("127.0.0.1", port)).map_err(|error| {
        AppError::new("ui", format!("could not bind 127.0.0.1:{port}: {error}"))
            .with_hint("Pick another port with `herald ui --port 0` for a free one.")
    })?;
    let bound_port = server
        .server_addr()
        .to_ip()
        .map_or(port, |address| address.port());
    let token = uuid::Uuid::new_v4().simple().to_string();
    let url = format!("http://127.0.0.1:{bound_port}/?token={token}");
    // The URL file is how a supervised `herald ui` (systemd user unit) hands
    // the per-run URL to the human; it carries the token, so it is written
    // 0600 and replaced atomically.
    let data_dir = resolve_data_dir(cli.data_dir.as_deref()).map_err(|error| {
        AppError::new(
            "ui",
            format!("could not resolve the data directory: {error}"),
        )
    })?;
    ensure_private_dir(&data_dir).map_err(|error| {
        AppError::new(
            "ui",
            format!("could not create the data directory: {error}"),
        )
    })?;
    write_ui_url(&data_dir, &url)?;
    let context = Context {
        data_dir: cli.data_dir.clone(),
        config: cli.config.clone(),
        token: token.clone(),
        port: bound_port,
        gate: Arc::new(Mutex::new(())),
    };

    println!("Herald UI  {url}");
    println!("Loopback only. Every API call needs the token. Ctrl+C stops it.");
    println!("Approvals and autopost are the same rails as the CLI.");
    let _ = std::io::stdout().flush();

    if open {
        open_browser(&url);
    }

    for request in server.incoming_requests() {
        let context = context.clone();
        std::thread::spawn(move || handle_request(request, &context));
    }
    Ok(0)
}

/// Write the per-run URL to `<data-dir>/ui-url`, replacing any previous
/// file atomically. The file carries the session token, so it is created
/// with mode 0600 on Unix before the rename.
fn write_ui_url(data_dir: &Path, url: &str) -> Result<(), AppError> {
    let target = data_dir.join("ui-url");
    let temporary = data_dir.join(format!(".ui-url.{}.tmp", uuid::Uuid::new_v4().simple()));
    let result = (|| -> std::io::Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(url.as_bytes())?;
        file.write_all(b"\n")?;
        match std::fs::rename(&temporary, &target) {
            Ok(()) => Ok(()),
            // Windows cannot replace an existing file with rename.
            Err(first) if target.exists() => {
                std::fs::remove_file(&target)?;
                std::fs::rename(&temporary, &target).map_err(|_| first)
            }
            Err(error) => Err(error),
        }
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temporary);
        return Err(AppError::new(
            "ui",
            format!("could not write {}: {error}", target.display()),
        ));
    }
    Ok(())
}

/// Launch `xdg-open` on the URL without waiting for it. Browser failures
/// are not the server's problem, so they are ignored.
#[cfg(unix)]
fn open_browser(url: &str) {
    let _ = Command::new("xdg-open")
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Browsers are only wired up on Unix; elsewhere the flag does nothing.
#[cfg(not(unix))]
fn open_browser(_url: &str) {}

fn handle_request(mut request: Request, context: &Context) {
    let url = request.url().to_string();
    let path = url.split('?').next().unwrap_or("/").to_string();
    let method = request.method().clone();

    let response = match (&method, path.as_str()) {
        (Method::Get, "/") => match query_param(&url, "token") {
            Some(token) if constant_time_eq(&token, &context.token) => {
                html_response(INDEX_HTML.replace("__TOKEN__", &context.token))
            }
            _ => text_response(
                403,
                "Open the URL printed by `herald ui`; it carries the session token.",
            ),
        },
        (Method::Get, "/ui.css") => asset_response(UI_CSS, "text/css; charset=utf-8"),
        (Method::Get, "/ui.js") => asset_response(UI_JS, "text/javascript; charset=utf-8"),
        (_, path) if path.starts_with("/api/") => {
            if authorized(&request, context) {
                let body = read_json_body(&mut request);
                api_response(&method, path, &url, &body, context)
            } else {
                json_response(&json!({
                    "ok": false,
                    "error": {"code": "unauthorized", "message": "missing or wrong session token"}
                }))
            }
        }
        _ => text_response(404, "not found"),
    };

    let _ = request.respond(response);
}

fn api_response(
    method: &Method,
    path: &str,
    url: &str,
    body: &Value,
    context: &Context,
) -> Response<std::io::Cursor<Vec<u8>>> {
    let result = match (method, path) {
        (Method::Get, "/api/meta") => Ok(envelope("meta", &meta(context))),
        (Method::Get, "/api/inbox") => Ok(context.run(&inbox_args(url))),
        (Method::Get, "/api/thread") => Ok(context.run(&thread_args(url))),
        (Method::Get, "/api/approvals") => Ok(context.run(&["approvals".to_string()])),
        (Method::Get, "/api/history") => Ok(context.run(&history_args(url))),
        (Method::Get, "/api/autopost") => {
            Ok(context.run(&["autopost".to_string(), "status".to_string()]))
        }
        (Method::Get, "/api/providers") => Ok(context.run(&["providers".to_string()])),
        (Method::Post, "/api/approve") => run_with_id(context, "approve", body),
        (Method::Post, "/api/reject") => run_with_id(context, "reject", body),
        (Method::Post, "/api/reconcile") => run_with_id(context, "reconcile", body),
        (Method::Post, "/api/post") => {
            post_action(body).map(|post| context.run_with_env(&post.args, &post.env))
        }
        (Method::Post, "/api/autopost") => {
            autopost_action(body).map(|action| context.run(&["autopost".to_string(), action]))
        }
        (Method::Post, "/api/ask") => ask_action(body).map(|args| context.run(&args)),
        _ => Err(ApiError::new("not_found", "unknown endpoint")),
    };

    match result {
        Ok(value) => json_response(&value),
        Err(error) => json_response(&json!({
            "ok": false,
            "error": {"code": error.code, "message": error.message}
        })),
    }
}

fn meta(context: &Context) -> Value {
    let config = context.run(&["config".to_string(), "list".to_string()]);
    let path = context.run(&["config".to_string(), "path".to_string()]);
    json!({
        "data_dir": context.data_dir.as_ref().map(|path| path.display().to_string()),
        "config_path": path
            .get("data")
            .and_then(|data| data.get("path"))
            .cloned(),
        "config": config.get("data").and_then(|data| data.get("config")).cloned(),
    })
}

fn inbox_args(url: &str) -> Vec<String> {
    let scope = query_param(url, "scope").unwrap_or_else(|| "all".to_string());
    let scope = match scope.as_str() {
        "all" | "feed" | "mentions" | "dms" | "notifications" => scope,
        _ => "all".to_string(),
    };
    let provider = query_param(url, "provider").unwrap_or_else(|| "fake".to_string());
    let limit = query_param(url, "limit")
        .and_then(|value| value.parse::<usize>().ok())
        .map_or(10, |value| value.clamp(1, 50));
    let mut args = vec![
        "inbox".to_string(),
        "--scope".to_string(),
        scope,
        "--provider".to_string(),
        provider,
        "--limit".to_string(),
        limit.to_string(),
    ];
    if query_param(url, "unread").as_deref() == Some("true") {
        args.push("--unread".to_string());
    }
    args
}

fn thread_args(url: &str) -> Vec<String> {
    let id = query_param(url, "id").unwrap_or_default();
    let provider = query_param(url, "provider").unwrap_or_else(|| "fake".to_string());
    vec!["thread".to_string(), id, "--provider".to_string(), provider]
}

fn history_args(url: &str) -> Vec<String> {
    let limit = query_param(url, "limit")
        .and_then(|value| value.parse::<usize>().ok())
        .map_or(20, |value| value.clamp(1, 50));
    vec![
        "history".to_string(),
        "--limit".to_string(),
        limit.to_string(),
    ]
}

fn autopost_action(body: &Value) -> Result<String, ApiError> {
    let action = body
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("status");
    match action {
        "status" | "on" | "off" => Ok(action.to_string()),
        _ => Err(ApiError::new(
            "invalid",
            "action must be on, off, or status",
        )),
    }
}

fn ask_action(body: &Value) -> Result<Vec<String>, ApiError> {
    let instruction = body
        .get("instruction")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.chars().count() <= 4_000)
        .ok_or_else(|| ApiError::new("invalid", "instruction is empty or too long"))?;
    let mut args = vec!["ask".to_string(), instruction.to_string()];
    if let Some(provider) = body.get("provider").and_then(Value::as_str)
        && !provider.is_empty()
    {
        args.push("--provider".to_string());
        args.push(provider.to_string());
    }
    if body
        .get("enable_send")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        args.push("--enable-send".to_string());
    }
    Ok(args)
}

/// The composed `ask "post exactly:"` invocation: argv plus the child-process
/// environment carrying the exact body.
struct PostInvocation {
    args: Vec<String>,
    env: Vec<(String, String)>,
}

/// Build the `ask "post exactly:"` invocation for the composer. The exact body
/// travels in the child environment (`HERALD_UI_BODY`), never argv; the CLI
/// reads it with `--body-env` and queues an approval. Nothing is sent here.
fn post_action(body: &Value) -> Result<PostInvocation, ApiError> {
    let text = body
        .get("body")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::new("invalid", "body is required"))?;
    let length = text.chars().count();
    if !(1..=crate::planner::POST_MAX_CHARS).contains(&length) {
        return Err(ApiError::new("invalid", "body must be 1 to 280 characters"));
    }
    let provider = match body.get("provider").and_then(Value::as_str) {
        None => "x",
        Some(raw) => {
            let trimmed = raw.trim();
            let valid = !trimmed.is_empty()
                && trimmed.chars().count() <= 32
                && trimmed
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphanumeric())
                && trimmed
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-');
            if !valid {
                return Err(ApiError::new(
                    "invalid",
                    "provider is not a known provider id",
                ));
            }
            trimmed
        }
    };
    let args = vec![
        "ask".to_string(),
        "post exactly:".to_string(),
        "--provider".to_string(),
        provider.to_string(),
        "--body-env".to_string(),
        "HERALD_UI_BODY".to_string(),
        "--enable-send".to_string(),
    ];
    Ok(PostInvocation {
        args,
        env: vec![("HERALD_UI_BODY".to_string(), text.to_string())],
    })
}

impl Context {
    fn run(&self, args: &[String]) -> Value {
        self.run_with_env(args, &[])
    }

    fn run_with_env(&self, args: &[String], env: &[(String, String)]) -> Value {
        let _guard = self
            .gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Ok(executable) = std::env::current_exe() else {
            return cli_error("exe", "could not resolve the herald binary");
        };
        let mut command = Command::new(executable);
        if let Some(data_dir) = &self.data_dir {
            command.arg("--data-dir").arg(data_dir);
        }
        if let Some(config) = &self.config {
            command.arg("--config").arg(config);
        }
        command.arg("--no-color").arg("--json").args(args);
        for (key, value) in env {
            command.env(key, value);
        }

        match command.output() {
            Ok(output) => {
                if let Ok(value) = serde_json::from_slice::<Value>(&output.stdout) {
                    return value;
                }
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stdout = String::from_utf8_lossy(&output.stdout);
                cli_error(
                    "output",
                    &format!(
                        "herald did not return JSON (exit {}). stderr: {} stdout: {}",
                        output.status,
                        tail(&stderr, 400),
                        tail(&stdout, 200)
                    ),
                )
            }
            Err(error) => cli_error("spawn", &format!("could not run herald: {error}")),
        }
    }
}

fn envelope(command: &str, data: &Value) -> Value {
    json!({"ok": true, "command": command, "data": data})
}

fn cli_error(code: &str, message: &str) -> Value {
    json!({"ok": false, "error": {"code": code, "message": message}})
}

#[derive(Debug)]
struct ApiError {
    code: &'static str,
    message: String,
}

impl ApiError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

fn read_json_body(request: &mut Request) -> Value {
    let mut raw = String::new();
    let _ = request
        .as_reader()
        .take(MAX_BODY_BYTES)
        .read_to_string(&mut raw);
    serde_json::from_str(&raw).unwrap_or(Value::Null)
}

fn authorized(request: &Request, context: &Context) -> bool {
    let token_ok = request
        .headers()
        .iter()
        .find(|header| header.field.equiv("X-Herald-Token"))
        .is_some_and(|header| constant_time_eq(header.value.as_str(), &context.token));
    if !token_ok {
        return false;
    }
    request
        .headers()
        .iter()
        .find(|header| header.field.equiv("Origin"))
        .is_none_or(|header| {
            let origin = header.value.as_str();
            origin == format!("http://127.0.0.1:{}", context.port)
                || origin == format!("http://localhost:{}", context.port)
        })
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0_u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

fn query_param(url: &str, key: &str) -> Option<String> {
    let (_, query) = url.split_once('?')?;
    for pair in query.split('&') {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        if percent_decode(name) == key {
            return Some(percent_decode(value));
        }
    }
    None
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    out.push(byte);
                    index += 3;
                    continue;
                }
                out.push(b'%');
                index += 1;
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn tail(value: &str, limit: usize) -> String {
    let trimmed = value.trim();
    let count = trimmed.chars().count();
    if count <= limit {
        return trimmed.to_string();
    }
    trimmed.chars().skip(count - limit).collect()
}

fn security_headers() -> Vec<Header> {
    [
        (
            "Content-Security-Policy",
            "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'",
        ),
        ("X-Content-Type-Options", "nosniff"),
        ("Referrer-Policy", "no-referrer"),
        ("X-Frame-Options", "DENY"),
        ("Cache-Control", "no-store"),
    ]
    .into_iter()
    .filter_map(|(name, value)| Header::from_bytes(name.as_bytes(), value.as_bytes()).ok())
    .collect()
}

fn html_response(body: String) -> Response<std::io::Cursor<Vec<u8>>> {
    let mut response = Response::from_string(body).with_status_code(200);
    for header in security_headers() {
        response.add_header(header);
    }
    if let Ok(header) = Header::from_bytes("Content-Type", "text/html; charset=utf-8") {
        response.add_header(header);
    }
    response
}

fn asset_response(body: &'static str, content_type: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    let mut response = Response::from_string(body).with_status_code(200);
    if let Ok(header) = Header::from_bytes("Content-Type", content_type) {
        response.add_header(header);
    }
    if let Ok(header) = Header::from_bytes("Cache-Control", "no-store") {
        response.add_header(header);
    }
    response
}

fn text_response(status: u16, body: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(body).with_status_code(status)
}

fn json_response(value: &Value) -> Response<std::io::Cursor<Vec<u8>>> {
    let mut response = Response::from_string(value.to_string()).with_status_code(200);
    if let Ok(header) = Header::from_bytes("Content-Type", "application/json") {
        response.add_header(header);
    }
    if let Ok(header) = Header::from_bytes("Cache-Control", "no-store") {
        response.add_header(header);
    }
    response
}

/// Accept an id from the UI and run the matching CLI command.
fn run_with_id(context: &Context, command: &str, body: &Value) -> Result<Value, ApiError> {
    let id = body
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty() && id.chars().count() <= 200)
        .ok_or_else(|| ApiError::new("invalid", "missing action id"))?;
    Ok(context.run(&[command.to_string(), id.to_string()]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> Context {
        Context {
            data_dir: None,
            config: None,
            token: "abc123".into(),
            port: 7878,
            gate: Arc::new(Mutex::new(())),
        }
    }

    #[test]
    fn token_comparison_is_exact() {
        let context = context();
        assert!(constant_time_eq("abc123", &context.token));
        assert!(!constant_time_eq("abc124", &context.token));
        assert!(!constant_time_eq("abc12", &context.token));
    }

    #[test]
    fn query_values_are_percent_decoded() {
        assert_eq!(
            query_param("/api/thread?id=mention%3A42&provider=x", "id").as_deref(),
            Some("mention:42")
        );
        assert_eq!(
            query_param("/api/inbox?scope=feed", "scope").as_deref(),
            Some("feed")
        );
        assert_eq!(query_param("/api/inbox", "scope"), None);
    }

    #[test]
    fn inbox_arguments_are_bounded_and_known() {
        let args = inbox_args("/api/inbox?scope=feed&provider=x&limit=999");
        assert!(args.contains(&"feed".to_string()));
        assert!(args.contains(&"50".to_string()));
        let args = inbox_args("/api/inbox?scope=evil&limit=0");
        assert!(args.contains(&"all".to_string()));
        assert!(args.contains(&"1".to_string()));
    }

    #[test]
    fn history_arguments_are_bounded_and_known() {
        let args = history_args("/api/history?limit=999");
        assert_eq!(args[0], "history");
        assert!(args.contains(&"--limit".to_string()));
        assert!(args.contains(&"50".to_string()));
        let args = history_args("/api/history?limit=0");
        assert!(args.contains(&"1".to_string()));
        let args = history_args("/api/history");
        assert!(args.contains(&"20".to_string()));
        let args = history_args("/api/history?limit=abc");
        assert!(args.contains(&"20".to_string()));
    }

    #[test]
    fn ask_requires_an_instruction_and_maps_flags() {
        let body =
            json!({"instruction": "post exactly: hello", "provider": "x", "enable_send": true});
        let args = ask_action(&body).unwrap();
        assert_eq!(args[0], "ask");
        assert!(args.contains(&"--enable-send".to_string()));
        assert!(args.contains(&"x".to_string()));
        assert!(ask_action(&json!({"instruction": "  "})).is_err());
    }

    #[test]
    fn the_page_has_a_token_slot_and_the_script_sends_it() {
        assert!(INDEX_HTML.contains("__TOKEN__"));
        assert!(UI_JS.contains("X-Herald-Token"));
        assert!(
            !UI_JS.contains("innerHTML"),
            "untrusted content must be textContent"
        );
    }

    #[test]
    fn post_validates_body_length_in_chars() {
        assert!(post_action(&json!({})).is_err());
        assert!(post_action(&json!({"body": ""})).is_err());
        assert!(post_action(&json!({"body": 42})).is_err());
        assert!(post_action(&json!({"body": "x".repeat(280)})).is_ok());
        assert!(post_action(&json!({"body": "x".repeat(281)})).is_err());
        // Counted in characters, not bytes.
        assert!(post_action(&json!({"body": "é".repeat(280)})).is_ok());
        assert!(post_action(&json!({"body": "é".repeat(281)})).is_err());
    }

    #[test]
    fn post_passes_the_exact_body_via_env_not_argv() {
        let body = "Ship it — \"today\". 🚀";
        let post = post_action(&json!({"body": body, "provider": "maildir"})).unwrap();
        let args = &post.args;
        let has_pair =
            |flag: &str, value: &str| args.windows(2).any(|w| w[0] == flag && w[1] == value);
        assert_eq!(args[0], "ask");
        assert!(args.contains(&"post exactly:".to_string()));
        assert!(has_pair("--provider", "maildir"));
        assert!(has_pair("--body-env", "HERALD_UI_BODY"));
        assert!(args.contains(&"--enable-send".to_string()));
        assert!(
            !args.iter().any(|arg| arg.contains(body)),
            "the body must never appear in argv"
        );
        assert_eq!(
            post.env,
            vec![("HERALD_UI_BODY".to_string(), body.to_string())]
        );
    }

    #[test]
    fn post_defaults_to_x_and_rejects_odd_providers() {
        let post = post_action(&json!({"body": "hello"})).unwrap();
        assert!(
            post.args
                .windows(2)
                .any(|w| w[0] == "--provider" && w[1] == "x")
        );
        assert!(post_action(&json!({"body": "hello", "provider": "--enable-send"})).is_err());
        assert!(post_action(&json!({"body": "hello", "provider": ""})).is_err());
    }

    #[test]
    fn autopost_actions_are_closed() {
        assert_eq!(autopost_action(&json!({"action": "off"})).unwrap(), "off");
        assert!(autopost_action(&json!({"action": "watch"})).is_err());
    }
}
