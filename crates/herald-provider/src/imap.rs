//! IMAP provider: read-only Gmail IMAP over TLS with app-password auth.
//!
//! The connection model is deliberately boring: one short-lived session per
//! operation (connect, log in, select, fetch, log out), executed on a
//! blocking worker so no async executor ever blocks on the socket. There is
//! no polling, no IDLE, no notification channel, and no persistent socket.
//!
//! Reads are header-first: `list_recent` fetches headers and flags only, and
//! bodies stay on the server until a thread is opened. Messages are threaded
//! by the first normalized `References` id (or the message's own id), so the
//! thread id does not depend on fetch order.
//!
//! Sending is not part of this provider: `prepare_reply` and `send_reply` are
//! `NotConfigured`. The only mutation the provider ever performs is the
//! `\Seen` flag via `STORE` when a thread is opened, and only when
//! `mark_read_on_open` is enabled (default true, mirroring the X adapter's
//! local seen-ledger; set false to keep reads side-effect free). Gmail app
//! passwords are the only supported credential: Gmail requires 2-step
//! verification to create one, and the plain password cannot be used for IMAP.
//!
//! Page content cannot steer anything here: there are no selectors, URLs, or
//! scripts — only protocol commands against constants. The folder name comes
//! from config, never from a message.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use mail_parser::MessageParser;

use herald_core::{
    MessageId, MessageQuery, MessageSummary, ProviderId, ProviderMessage, Thread, ThreadId,
    Untrusted,
};

use crate::mail::{normalize_message_id, sender_of};
use crate::{
    CommunicationProvider, ExecutionReceipt, PreparedAction, ProviderError, ProviderHealth,
};
/// A connected IMAP stream. The blanket impl exists because `Box<dyn Read +
/// Write + Send>` is not a legal trait object (`Send` is an auto trait); this
/// helper trait names the one shape the session accepts.
trait ImapStream: Read + Write + Send {}
impl<T: Read + Write + Send> ImapStream for T {}
type SessionClient = imap::Session<Box<dyn ImapStream + 'static>>;

/// Connect timeout for the socket, and the all-in budget for one operation.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const OP_TIMEOUT: Duration = Duration::from_secs(30);
/// Upper bound on header fetches per read, so a huge mailbox cannot make a
/// read allocate without limit.
const MAX_LIST_LIMIT: usize = 500;
/// Body-fetch bound per thread: a runaway thread never turns one `get_thread`
/// into fetching a whole export.
const MAX_THREAD_MESSAGES: usize = 32;
/// Upper bound on retained body bytes per message.
const MAX_BODY_BYTES: usize = 256 * 1024;
/// Default page size when the caller asks for `limit = 0`.
const DEFAULT_LIMIT: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImapProviderConfig {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    pub folder: String,
    pub use_tls: bool,
    pub mark_read_on_open: bool,
}

impl Default for ImapProviderConfig {
    fn default() -> Self {
        Self {
            host: "imap.gmail.com".to_string(),
            port: 993,
            username: None,
            password: None,
            folder: "INBOX".to_string(),
            use_tls: true,
            mark_read_on_open: true,
        }
    }
}

/// IMAP adapter. Stateless besides config: every operation opens its own
/// session, so there is no connection to hold across the process.
pub struct ImapProvider {
    config: ImapProviderConfig,
}

impl ImapProvider {
    #[must_use]
    pub fn new(config: ImapProviderConfig) -> Self {
        Self { config }
    }

    /// Run the blocking session on a worker with an all-in timeout.
    async fn run<F, T>(&self, operation: F) -> Result<T, ProviderError>
    where
        F: FnOnce(&mut SessionClient) -> Result<T, ProviderError> + Send + 'static,
        T: Send + 'static,
    {
        let config = self.config.clone();
        let joined = tokio::time::timeout(OP_TIMEOUT, async move {
            tokio::task::spawn_blocking(move || {
                let mut client = connect(&config)?;
                let result = operation(&mut client);
                let _ = client.logout();
                result
            })
            .await
        })
        .await;
        match joined {
            Err(_) => Err(ProviderError::Timeout),
            Ok(Err(join_error)) => Err(ProviderError::Other(format!(
                "imap worker panicked: {join_error}"
            ))),
            Ok(Ok(Err(error))) => Err(error),
            Ok(Ok(Ok(result))) => Ok(result),
        }
    }
}

/// One parsed message from the header fetch.
#[derive(Debug, Clone)]
struct ImapMessage {
    uid: u32,
    id: MessageId,
    thread_id: ThreadId,
    sender: String,
    subject: String,
    timestamp_ms: i64,
    unread: bool,
}

#[async_trait]
impl CommunicationProvider for ImapProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Imap
    }

    async fn health(&self) -> ProviderHealth {
        let folder = self.config.folder.clone();
        let result = self
            .run(move |client| {
                client
                    .examine(&folder)
                    .map_err(|error| map_session_error(&error))?;
                Ok::<(), ProviderError>(())
            })
            .await;
        match result {
            Ok(()) => ProviderHealth::Ready,
            Err(ProviderError::NotAuthenticated) => ProviderHealth::NotAuthenticated,
            Err(error) => ProviderHealth::Unavailable(error.to_string()),
        }
    }

    async fn list_recent(
        &self,
        query: &MessageQuery,
    ) -> Result<Vec<MessageSummary>, ProviderError> {
        let bounded_limit = query.limit.min(MAX_LIST_LIMIT);
        let limit = if bounded_limit == 0 {
            DEFAULT_LIMIT
        } else {
            bounded_limit
        };
        let folder = self.config.folder.clone();
        let mut messages = self
            .run(move |client| {
                let mailbox = client
                    .examine(&folder)
                    .map_err(|error| map_session_error(&error))?;
                fetch_messages(client, mailbox.exists, limit)
            })
            .await?;

        let search = crate::search_needle(query);
        messages.retain(|message| {
            if query.unread_only && !message.unread {
                return false;
            }
            crate::matches_search(
                search.as_ref(),
                &[
                    message.sender.as_str(),
                    message.subject.as_str(),
                    message.id.as_str(),
                ],
            )
        });

        messages.sort_by(|left, right| {
            right
                .timestamp_ms
                .cmp(&left.timestamp_ms)
                .then_with(|| right.uid.cmp(&left.uid))
        });
        messages.dedup_by(|left, right| left.id == right.id);
        messages.truncate(limit);

        Ok(messages
            .into_iter()
            .map(|message| MessageSummary {
                id: message.id,
                thread_id: message.thread_id,
                sender: message.sender,
                subject: message.subject,
                timestamp_ms: message.timestamp_ms,
                unread: message.unread,
            })
            .collect())
    }

    async fn get_thread(&self, id: &ThreadId) -> Result<Thread, ProviderError> {
        let folder = self.config.folder.clone();
        let mark_read = self.config.mark_read_on_open;
        let wanted = id.clone();
        let thread = self
            .run(move |client| {
                let mailbox = client
                    .examine(&folder)
                    .map_err(|error| map_session_error(&error))?;
                let mut matched: Vec<ImapMessage> =
                    fetch_messages(client, mailbox.exists, MAX_LIST_LIMIT)?
                        .into_iter()
                        .filter(|message| message.thread_id == wanted)
                        .collect();
                if matched.is_empty() {
                    return Err(ProviderError::NotFound);
                }
                matched.sort_by(|left, right| {
                    left.timestamp_ms
                        .cmp(&right.timestamp_ms)
                        .then_with(|| left.uid.cmp(&right.uid))
                });
                matched.truncate(MAX_THREAD_MESSAGES);

                if mark_read && matched.iter().any(|message| message.unread) {
                    // Upgrade the examine (read-only) to a writable select on
                    // the same session, then flag. Best effort: a failed
                    // STORE must not lose the messages that were just read.
                    let _ = client.select(&folder);
                    let uids: Vec<String> = matched
                        .iter()
                        .filter(|message| message.unread)
                        .map(|message| message.uid.to_string())
                        .collect();
                    if let Err(error) = client.store(uids.join(","), "+FLAGS (\\Seen)") {
                        tracing::warn!(%error, "could not mark the thread as read");
                    }
                }

                let uid_list: Vec<String> = matched.iter().map(|m| m.uid.to_string()).collect();
                let bodies = client
                    .uid_fetch(uid_list.join(","), "BODY.PEEK[]")
                    .map_err(|error| map_session_error(&error))?;

                let mut by_uid: std::collections::HashMap<u32, String> =
                    std::collections::HashMap::new();
                for body in &bodies {
                    let raw = body.body().unwrap_or(&[]);
                    by_uid.insert(body.uid.unwrap_or(body.message), extract_body(raw));
                }

                let subject = matched
                    .first()
                    .map(|message| message.subject.clone())
                    .unwrap_or_default();
                let messages = matched
                    .into_iter()
                    .map(|message| {
                        let body = by_uid.remove(&message.uid).unwrap_or_default();
                        ProviderMessage {
                            id: message.id,
                            thread_id: wanted.clone(),
                            sender: message.sender,
                            subject: message.subject,
                            body: Untrusted(body),
                            timestamp_ms: message.timestamp_ms,
                            unread: message.unread,
                        }
                    })
                    .collect();

                Ok(Thread {
                    id: wanted.clone(),
                    provider: ProviderId::Imap,
                    subject,
                    messages,
                })
            })
            .await?;
        Ok(thread)
    }

    async fn prepare_reply(
        &self,
        _id: &ThreadId,
        _body: &str,
        _client_token: &str,
    ) -> Result<PreparedAction, ProviderError> {
        Err(ProviderError::NotConfigured(
            "imap is read-only; replies via IMAP are not supported".into(),
        ))
    }

    async fn send_reply(
        &self,
        _action: &PreparedAction,
    ) -> Result<ExecutionReceipt, ProviderError> {
        Err(ProviderError::NotConfigured(
            "imap is read-only; sends are not supported".into(),
        ))
    }

    async fn find_sent(
        &self,
        _thread: &ThreadId,
        _client_token: &str,
    ) -> Result<Option<ExecutionReceipt>, ProviderError> {
        Ok(None)
    }
}

/// Open a session and log in. Handshake and login failures are deliberately
/// distinct: bad credentials are `NotAuthenticated`, everything else is `Other`.
fn connect(config: &ImapProviderConfig) -> Result<SessionClient, ProviderError> {
    let username = config
        .username
        .clone()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ProviderError::NotConfigured("imap.username is not set".to_string()))?;
    let password = config
        .password
        .clone()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ProviderError::NotConfigured("imap password is not set".to_string()))?;

    let mut addresses = (config.host.as_str(), config.port)
        .to_socket_addrs()
        .map_err(|error| ProviderError::Other(format!("resolving {}: {error}", config.host)))?;
    let mut stream = None;
    let mut last_error = None;
    for address in addresses.by_ref() {
        match TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) {
            Ok(connected) => {
                stream = Some(connected);
                break;
            }
            Err(error) => last_error = Some(error),
        }
    }
    let stream = stream.ok_or_else(|| {
        ProviderError::Other(format!(
            "connecting to {}:{}: {}",
            config.host,
            config.port,
            last_error.map_or_else(|| "no address".to_string(), |error| error.to_string(),)
        ))
    })?;

    let stream: Box<dyn ImapStream> = if config.use_tls {
        Box::new(tls_wrap(stream, &config.host)?)
    } else {
        Box::new(stream)
    };

    let client = imap::Client::new(stream);
    let session = client
        .login(&username, &password)
        .map_err(|(_error, _client)| ProviderError::NotAuthenticated)?;
    Ok(session)
}

fn tls_wrap(stream: TcpStream, host: &str) -> Result<Box<dyn ImapStream + 'static>, ProviderError> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let server_name = rustls::pki_types::ServerName::try_from(host.to_string()).map_err(|_| {
        ProviderError::NotConfigured(format!("imap.host {host:?} is not a valid hostname"))
    })?;
    let connection = rustls::ClientConnection::new(Arc::new(config), server_name)
        .map_err(|error| ProviderError::Other(format!("TLS setup: {error}")))?;
    Ok(Box::new(rustls::StreamOwned::new(connection, stream)))
}

/// Fetch headers+flags for the last `limit` messages of an examined mailbox.
fn fetch_messages(
    client: &mut SessionClient,
    exists: u32,
    limit: usize,
) -> Result<Vec<ImapMessage>, ProviderError> {
    if exists == 0 {
        return Ok(Vec::new());
    }
    let end = exists;
    let start = exists.saturating_sub(u32::try_from(limit).unwrap_or(u32::MAX)) + 1;
    let range = format!("{start}:{end}");
    let fetches = client
        .fetch(
            &range,
            "UID FLAGS BODY.PEEK[HEADER.FIELDS (MESSAGE-ID IN-REPLY-TO REFERENCES FROM SUBJECT DATE)]",
        )
        .map_err(|error| map_session_error(&error))?;
    let mut messages = Vec::new();
    for fetch in &fetches {
        let Some(raw_header) = fetch.header() else {
            continue;
        };
        let Some(message) = parse_header(raw_header) else {
            continue;
        };
        let uid = fetch.uid.unwrap_or(fetch.message);
        let unread = !fetch
            .flags()
            .iter()
            .any(|flag| matches!(flag, imap::types::Flag::Seen));
        messages.push(ImapMessage {
            uid,
            unread,
            ..message
        });
    }
    Ok(messages)
}

/// Parse one header block. Garbage or header-less blocks are skipped, never
/// guessed (same rule as the maildir adapter).
fn parse_header(raw: &[u8]) -> Option<ImapMessage> {
    let message = MessageParser::new().parse(raw)?;
    if !message.headers().has_known() {
        return None;
    }
    let id = message
        .message_id()
        .and_then(normalize_message_id)
        .unwrap_or_else(|| "uid:unknown".to_string());
    let mut references: Vec<String> = Vec::new();
    for list in [message.references(), message.in_reply_to()] {
        let Some(list) = list else { continue };
        for reference in list.iter() {
            if let Some(reference) = normalize_message_id(reference)
                && !references.contains(&reference)
            {
                references.push(reference);
            }
        }
    }
    let thread_id = references.first().cloned().unwrap_or_else(|| id.clone());
    let (sender, _address) = sender_of(&message);
    let subject = message
        .subject()
        .map(str::trim)
        .filter(|subject| !subject.is_empty())
        .map_or_else(|| "(no subject)".to_string(), ToString::to_string);
    let timestamp_ms = message
        .date()
        .map_or(0, |date| date.to_timestamp().saturating_mul(1000));

    Some(ImapMessage {
        uid: 0,
        id: MessageId::new(id),
        thread_id: ThreadId::new(thread_id),
        sender,
        subject,
        timestamp_ms,
        unread: false,
    })
}

/// First text/plain part of a full message, bounded. HTML-only mail yields
/// an empty body, never markup presented as text.
fn extract_body(raw: &[u8]) -> String {
    let Some(message) = MessageParser::new().parse(raw) else {
        return String::new();
    };
    if !message.headers().has_known() {
        return String::new();
    }
    let text = message
        .body_text(0)
        .map_or_else(String::new, std::borrow::Cow::into_owned);
    if text.len() > MAX_BODY_BYTES {
        text.chars().take(MAX_BODY_BYTES).collect()
    } else {
        text
    }
}

fn map_session_error(error: &imap::error::Error) -> ProviderError {
    ProviderError::Other(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured() -> ImapProviderConfig {
        ImapProviderConfig {
            username: Some("me@gmail.com".into()),
            password: Some("app-password".into()),
            ..ImapProviderConfig::default()
        }
    }

    #[tokio::test]
    async fn writes_are_refused_without_a_network_round_trip() {
        let provider = ImapProvider::new(configured());
        let thread = ThreadId::new("abc@example.com");
        let action = PreparedAction::new(ProviderId::Imap, thread.clone(), "hi", "tok");
        assert!(matches!(
            provider.prepare_reply(&thread, "hi", "tok").await,
            Err(ProviderError::NotConfigured(_))
        ));
        assert!(matches!(
            provider.send_reply(&action).await,
            Err(ProviderError::NotConfigured(_))
        ));
        assert!(provider.find_sent(&thread, "tok").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn missing_credentials_never_touch_the_network() {
        let provider = ImapProvider::new(ImapProviderConfig::default());
        // No username: the adapter refuses before connecting.
        assert!(matches!(
            provider.health().await,
            ProviderHealth::Unavailable(_)
        ));
    }

    #[tokio::test]
    async fn a_refused_connection_is_unavailable_not_a_crash() {
        let mut config = configured();
        config.host = "127.0.0.1".to_string();
        // Port 1 is reserved; nothing listens there.
        config.port = 1;
        let provider = ImapProvider::new(config);
        assert!(matches!(
            provider.health().await,
            ProviderHealth::Unavailable(_)
        ));
        assert!(matches!(
            provider.list_recent(&MessageQuery::recent(5)).await,
            Err(ProviderError::Other(_))
        ));
    }

    #[test]
    fn message_ids_normalize_like_the_maildir_adapter() {
        assert_eq!(
            normalize_message_id("<abc@example.com>"),
            Some("abc@example.com".to_string())
        );
        assert_eq!(normalize_message_id("  "), None);
        assert_eq!(normalize_message_id("<a\nb>"), None);
    }

    #[test]
    fn sender_formatting_matches_maildir() {
        let raw = b"From: Sam Founder <sam@example.com>\r\nSubject: hi\r\n\r\n";
        let message = MessageParser::new().parse(raw).unwrap();
        let (sender, address) = sender_of(&message);
        assert_eq!(sender, "Sam Founder <sam@example.com>");
        assert_eq!(address.as_deref(), Some("sam@example.com"));
    }

    #[test]
    fn thread_root_is_the_first_reference() {
        let raw = b"Message-ID: <reply@example.com>\r\nReferences: <root@example.com> <mid@example.com>\r\nSubject: Re: hi\r\n\r\n";
        let message = parse_header(raw).unwrap();
        assert_eq!(message.id.as_str(), "reply@example.com");
        assert_eq!(message.thread_id.as_str(), "root@example.com");
    }

    #[test]
    fn a_reply_without_references_is_its_own_thread() {
        let raw = b"Message-ID: <solo@example.com>\r\nSubject: hi\r\n\r\n";
        let message = parse_header(raw).unwrap();
        assert_eq!(message.id.as_str(), "solo@example.com");
        assert_eq!(message.thread_id.as_str(), "solo@example.com");
    }

    #[test]
    fn header_only_bodies_do_not_leak_markup() {
        let raw = b"Message-ID: <m@example.com>\r\nSubject: hi\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nplain text body\r\n";
        assert_eq!(extract_body(raw), "plain text body\r\n");
    }

    #[test]
    fn body_extraction_bounds_runaway_messages() {
        let mut raw = Vec::from(&b"Message-ID: <m@example.com>\r\nSubject: hi\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n"[..]);
        raw.extend(std::iter::repeat_n(b'x', MAX_BODY_BYTES + 100));
        let body = extract_body(&raw);
        assert!(body.chars().count() <= MAX_BODY_BYTES);
    }
}
