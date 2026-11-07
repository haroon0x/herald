//! Local Maildir provider: read from `new/` and `cur/`, send over SMTP, and
//! keep a verified copy in the Sent maildir.
//!
//! Message content is untrusted data. It is parsed into plain text for display
//! and never interpreted as instructions or executed. Header values are
//! sanitized before they are written into an outbound message.
//!
//! Delivery is idempotent on [`PreparedAction::client_token`]. The Sent copy
//! carries an `X-Herald-Token` header, which is how a retry or a reconciliation
//! after a timeout recognizes a message that already left the machine.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use mail_parser::MessageParser;
use sha2::{Digest, Sha256};

use herald_core::{
    MessageId, MessageQuery, MessageSummary, ProviderId, ProviderMessage, Thread, ThreadId,
    Untrusted, body_hash, time::now_ms,
};

use crate::mail::{normalize_message_id, sender_of};
use crate::{
    CommunicationProvider, ExecutionReceipt, PreparedAction, ProviderError, ProviderHealth,
    require_not_empty_body,
};

/// Files larger than this are ignored rather than loaded into memory.
const MAX_MESSAGE_BYTES: u64 = 10 * 1024 * 1024;

/// Upper bound on generated `References` identifiers.
const MAX_REFERENCES: usize = 20;

/// Maildir info separator. Unix uses `:`; Windows filenames cannot contain it,
/// so the provider writes and accepts `;2,` there as well.
const FLAG_SEPARATOR: char = if cfg!(windows) { ';' } else { ':' };

/// SMTP command timeout used by the real transport.
const SMTP_TIMEOUT: Duration = Duration::from_secs(30);

/// How TLS is established for outbound SMTP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmtpSecurity {
    /// Plaintext connection upgraded with `STARTTLS` before credentials are
    /// used.
    Starttls,
    /// Implicit TLS (SMTPS).
    Tls,
    /// No TLS. Only sensible for a local relay.
    None,
}

/// SMTP settings for outbound mail.
///
/// `password` is resolved by the caller (normally from an environment
/// variable). Its `Debug` representation is redacted.
#[derive(Clone)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    pub security: SmtpSecurity,
}

impl fmt::Debug for SmtpConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SmtpConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("security", &self.security)
            .finish()
    }
}

/// Configuration for a Maildir inbox and, optionally, an SMTP relay.
#[derive(Debug, Clone)]
pub struct MaildirConfig {
    /// INBOX maildir root containing `cur/`, `new/`, and `tmp/`.
    pub root: PathBuf,
    /// Sent maildir root. Defaults to `<root>/.Sent` and is created on the
    /// first successful send.
    pub sent_path: Option<PathBuf>,
    /// Outbound `From` value, for example `Me <me@example.com>`.
    pub from: Option<String>,
    /// SMTP relay. Without it the maildir is read-only.
    pub smtp: Option<SmtpConfig>,
}

impl MaildirConfig {
    fn sent_root(&self) -> PathBuf {
        self.sent_path
            .clone()
            .unwrap_or_else(|| self.root.join(".Sent"))
    }
}

/// Outbound delivery, isolated so tests never need a live SMTP server.
pub trait MailTransport: Send + Sync + fmt::Debug {
    fn deliver(&self, from: &str, to: &str, raw: &[u8]) -> Result<(), ProviderError>;
}

/// Transport used when the provider is deliberately read-only.
#[derive(Debug)]
struct ReadOnlyTransport {
    error: ProviderError,
}

impl MailTransport for ReadOnlyTransport {
    fn deliver(&self, _from: &str, _to: &str, _raw: &[u8]) -> Result<(), ProviderError> {
        Err(self.error.clone())
    }
}

/// Real SMTP transport backed by `lettre`.
#[derive(Debug)]
struct SmtpTransport {
    transport: lettre::SmtpTransport,
}

impl SmtpTransport {
    fn new(config: &SmtpConfig) -> Result<Self, ProviderError> {
        let mut builder = lettre::SmtpTransport::builder_dangerous(&config.host)
            .port(config.port)
            .timeout(Some(SMTP_TIMEOUT));

        match config.security {
            SmtpSecurity::Tls => {
                let parameters =
                    lettre::transport::smtp::client::TlsParameters::new_rustls(config.host.clone())
                        .map_err(|_| {
                            ProviderError::NotConfigured("SMTP TLS could not be configured".into())
                        })?;
                builder = builder.tls(lettre::transport::smtp::client::Tls::Wrapper(parameters));
            }
            SmtpSecurity::Starttls => {
                let parameters =
                    lettre::transport::smtp::client::TlsParameters::new_rustls(config.host.clone())
                        .map_err(|_| {
                            ProviderError::NotConfigured("SMTP TLS could not be configured".into())
                        })?;
                builder = builder.tls(lettre::transport::smtp::client::Tls::Required(parameters));
            }
            SmtpSecurity::None => {
                builder = builder.tls(lettre::transport::smtp::client::Tls::None);
            }
        }

        if let Some(username) = &config.username {
            builder =
                builder.credentials(lettre::transport::smtp::authentication::Credentials::new(
                    username.clone(),
                    config.password.clone().unwrap_or_default(),
                ));
        }

        Ok(Self {
            transport: builder.build(),
        })
    }
}

impl MailTransport for SmtpTransport {
    fn deliver(&self, from: &str, to: &str, raw: &[u8]) -> Result<(), ProviderError> {
        use lettre::Transport as _;

        let from = from
            .parse::<lettre::Address>()
            .map_err(|_| ProviderError::Other("invalid sender address".into()))?;
        let to = to
            .parse::<lettre::Address>()
            .map_err(|_| ProviderError::Other("invalid recipient address".into()))?;
        let envelope = lettre::address::Envelope::new(Some(from), vec![to])
            .map_err(|_| ProviderError::Other("invalid SMTP envelope".into()))?;

        self.transport
            .send_raw(&envelope, raw)
            .map(|_response| ())
            .map_err(|error| map_smtp_error(&error))
    }
}

fn map_smtp_error(error: &lettre::transport::smtp::Error) -> ProviderError {
    if error.is_timeout() {
        return ProviderError::Timeout;
    }

    let text = error.to_string();
    let lowercase = text.to_ascii_lowercase();
    if lowercase.contains("auth") || lowercase.contains("credential") {
        return ProviderError::NotAuthenticated;
    }
    if error.is_transient() {
        return ProviderError::Other(format!("SMTP transient failure: {text}"));
    }
    ProviderError::Other(format!("SMTP delivery failed: {text}"))
}

/// Local Maildir provider.
pub struct MaildirProvider {
    config: MaildirConfig,
    transport: Arc<dyn MailTransport>,
}

impl MaildirProvider {
    /// Construct a provider without doing any I/O.
    ///
    /// When `config.smtp` is `None` the provider is read-only. A broken SMTP
    /// configuration is reported on the first send, not here, so constructing a
    /// provider for reads can never fail.
    #[must_use]
    pub fn new(config: MaildirConfig) -> Self {
        let transport: Arc<dyn MailTransport> = match &config.smtp {
            Some(smtp) => match SmtpTransport::new(smtp) {
                Ok(transport) => Arc::new(transport),
                Err(error) => Arc::new(ReadOnlyTransport { error }),
            },
            None => Arc::new(ReadOnlyTransport {
                error: ProviderError::NotConfigured(
                    "SMTP is not configured; this maildir is read-only".into(),
                ),
            }),
        };
        Self { config, transport }
    }

    /// Construct a provider with an explicit transport (used by tests and by
    /// callers that provide their own SMTP integration).
    #[must_use]
    pub fn with_transport(config: MaildirConfig, transport: Arc<dyn MailTransport>) -> Self {
        Self { config, transport }
    }

    fn sent_root(&self) -> PathBuf {
        self.config.sent_root()
    }

    fn load_inbox(&self) -> Vec<RawMessage> {
        scan_inbox(&self.config.root)
    }

    /// Load every message of one thread, oldest first.
    fn load_thread(&self, id: &ThreadId) -> Result<Vec<RawMessage>, ProviderError> {
        let mut messages: Vec<RawMessage> = self
            .load_inbox()
            .into_iter()
            .filter(|message| message.thread_id == id.as_str())
            .collect();
        if messages.is_empty() {
            return Err(ProviderError::NotFound);
        }
        messages.sort_by(|a, b| {
            a.timestamp_ms
                .cmp(&b.timestamp_ms)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(messages)
    }

    /// Append the exact delivered bytes to the Sent maildir.
    fn append_to_sent(&self, raw: &[u8]) -> std::io::Result<PathBuf> {
        let root = self.sent_root();
        for folder in ["cur", "new", "tmp"] {
            fs::create_dir_all(root.join(folder))?;
        }

        let name = unique_name();
        let tmp_path = root.join("tmp").join(&name);
        let cur_path = root.join("cur").join(format!("{name}{FLAG_SEPARATOR}2,S"));

        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)?;
        file.write_all(raw)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp_path, &cur_path)?;
        Ok(cur_path)
    }

    fn find_sent_sync(&self, thread: &ThreadId, client_token: &str) -> Option<ExecutionReceipt> {
        let sent = self.sent_root();
        if sent.is_dir()
            && let Some(receipt) = find_token_in_folder(&sent, thread, client_token)
        {
            return Some(receipt);
        }

        // If the Sent folder lives inside the INBOX, also look there. This
        // covers configurations where sent mail is filed under the inbox.
        if sent.starts_with(&self.config.root) && sent != self.config.root {
            let messages = self.load_inbox();
            if let Some(message) = messages
                .into_iter()
                .find(|message| message.token.as_deref() == Some(client_token))
            {
                return Some(receipt_for(message, thread));
            }
        }

        None
    }
}

#[async_trait]
impl CommunicationProvider for MaildirProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Maildir
    }

    async fn health(&self) -> ProviderHealth {
        let root = &self.config.root;
        let Ok(metadata) = fs::metadata(root) else {
            return ProviderHealth::Unavailable(format!(
                "maildir root does not exist: {}",
                root.display()
            ));
        };
        if !metadata.is_dir() {
            return ProviderHealth::Unavailable(format!(
                "maildir root is not a directory: {}",
                root.display()
            ));
        }
        for folder in ["cur", "new"] {
            if root.join(folder).is_dir() {
                return ProviderHealth::Ready;
            }
        }
        ProviderHealth::Unavailable(format!(
            "maildir is missing cur/ and new/: {}",
            root.display()
        ))
    }

    async fn list_recent(
        &self,
        query: &MessageQuery,
    ) -> Result<Vec<MessageSummary>, ProviderError> {
        let needle = crate::search_needle(query);
        let mut summaries: Vec<MessageSummary> = self
            .load_inbox()
            .into_iter()
            .filter(|message| !query.unread_only || message.unread)
            .filter(|message| {
                crate::matches_search(
                    needle.as_ref(),
                    &[
                        message.sender.as_str(),
                        message.subject.as_str(),
                        message.body_text.as_str(),
                    ],
                )
            })
            .map(|message| MessageSummary {
                id: MessageId::new(message.id),
                thread_id: ThreadId::new(message.thread_id),
                sender: message.sender,
                subject: message.subject,
                timestamp_ms: message.timestamp_ms,
                unread: message.unread,
            })
            .collect();

        summaries.sort_by(|a, b| {
            b.timestamp_ms
                .cmp(&a.timestamp_ms)
                .then_with(|| a.id.0.cmp(&b.id.0))
        });
        summaries.truncate(crate::effective_limit(query.limit));
        Ok(summaries)
    }

    async fn get_thread(&self, id: &ThreadId) -> Result<Thread, ProviderError> {
        let messages = self.load_thread(id)?;
        let subject = messages
            .first()
            .map(|message| message.subject.clone())
            .unwrap_or_default();
        Ok(Thread {
            id: id.clone(),
            provider: ProviderId::Maildir,
            subject,
            messages: messages
                .into_iter()
                .map(RawMessage::into_provider_message)
                .collect(),
        })
    }

    async fn prepare_reply(
        &self,
        id: &ThreadId,
        body: &str,
        client_token: &str,
    ) -> Result<PreparedAction, ProviderError> {
        require_not_empty_body(body, "empty reply body")?;
        // Verify the thread exists without touching the mailbox.
        self.load_thread(id)?;
        Ok(PreparedAction::new(
            ProviderId::Maildir,
            id.clone(),
            body,
            client_token,
        ))
    }

    async fn send_reply(&self, action: &PreparedAction) -> Result<ExecutionReceipt, ProviderError> {
        // Idempotency: the token is recorded in the Sent maildir. A retry that
        // finds it must not deliver again.
        if let Some(receipt) = self.find_sent_sync(&action.thread_id, &action.client_token) {
            return Ok(receipt);
        }

        if self.config.smtp.is_none() {
            return Err(ProviderError::NotConfigured(
                "SMTP is not configured; this maildir is read-only".into(),
            ));
        }

        let from_header =
            self.config.from.as_deref().ok_or_else(|| {
                ProviderError::NotConfigured("maildir.from is not configured".into())
            })?;
        if has_line_break(from_header) {
            return Err(ProviderError::Other(
                "maildir.from must not contain a line break".into(),
            ));
        }
        let from_address = extract_address(from_header).ok_or_else(|| {
            ProviderError::NotConfigured("maildir.from is not a valid address".into())
        })?;

        if action.client_token.chars().any(char::is_control) {
            return Err(ProviderError::Other(
                "client token must not contain control characters".into(),
            ));
        }

        let thread = self.load_thread(&action.thread_id)?;
        let recipient = choose_recipient(&thread, &from_address).ok_or(ProviderError::NotFound)?;
        if has_line_break(&recipient) {
            return Err(ProviderError::Other(
                "recipient address must not contain a line break".into(),
            ));
        }

        let base_subject = thread
            .first()
            .map_or_else(|| "(no subject)".to_string(), |m| m.subject.clone());
        let subject = sanitize_header_value(&reply_subject(&base_subject));

        let latest = thread.last();
        let mut references: Vec<String> = latest.map(|m| m.references.clone()).unwrap_or_default();
        if let Some(parent) = latest {
            push_unique(&mut references, parent.id.clone());
        }
        references.retain(|reference| !reference.is_empty());
        if references.len() > MAX_REFERENCES {
            let drop = references.len() - MAX_REFERENCES;
            references.drain(..drop);
        }
        let in_reply_to = latest.map(|message| message.id.clone());

        let sent_at_ms = now_ms();
        let message_id = outbound_message_id(&action.client_token);
        let raw = build_raw_message(&OutboundMessage {
            from_header,
            recipient: &recipient,
            subject: &subject,
            in_reply_to: in_reply_to.as_deref(),
            references: &references,
            message_id: &message_id,
            client_token: &action.client_token,
            body: &action.body,
            sent_at_ms,
        })?;

        // Deliver first. A transport error is not retried; the caller decides
        // whether it is `failed` or `uncertain`.
        self.transport.deliver(&from_address, &recipient, &raw)?;

        let verified = match self.append_to_sent(&raw) {
            Ok(path) => verify_sent_file(&path, &action.client_token, &action.body),
            Err(_) => false,
        };

        Ok(ExecutionReceipt {
            provider: ProviderId::Maildir,
            thread_id: action.thread_id.clone(),
            provider_message_id: MessageId::new(message_id),
            body_hash: action.body_hash.clone(),
            sent_at_ms,
            verified,
        })
    }

    async fn find_sent(
        &self,
        thread: &ThreadId,
        client_token: &str,
    ) -> Result<Option<ExecutionReceipt>, ProviderError> {
        Ok(self.find_sent_sync(thread, client_token))
    }
}

/// A parsed message plus the metadata needed for display and threading.
#[derive(Debug, Clone)]
struct RawMessage {
    id: String,
    thread_id: String,
    sender: String,
    sender_address: Option<String>,
    subject: String,
    body_text: String,
    timestamp_ms: i64,
    unread: bool,
    references: Vec<String>,
    token: Option<String>,
}

impl RawMessage {
    fn into_provider_message(self) -> ProviderMessage {
        ProviderMessage {
            id: MessageId::new(self.id),
            thread_id: ThreadId::new(self.thread_id),
            sender: self.sender,
            subject: self.subject,
            body: Untrusted(self.body_text),
            timestamp_ms: self.timestamp_ms,
            unread: self.unread,
        }
    }
}

fn scan(maildir_root: &Path, sent: bool) -> Vec<RawMessage> {
    let mut by_id: BTreeMap<String, RawMessage> = BTreeMap::new();
    scan_folder(&maildir_root.join("new"), true, sent, &mut by_id);
    scan_folder(&maildir_root.join("cur"), false, sent, &mut by_id);
    let mut messages: Vec<RawMessage> = by_id.into_values().collect();
    if !sent {
        assign_thread_ids(&mut messages);
    }
    messages
}

fn scan_inbox(root: &Path) -> Vec<RawMessage> {
    scan(root, false)
}

fn scan_sent(root: &Path) -> Vec<RawMessage> {
    scan(root, true)
}

fn find_token_in_folder(
    root: &Path,
    thread: &ThreadId,
    client_token: &str,
) -> Option<ExecutionReceipt> {
    scan_sent(root).into_iter().find_map(|message| {
        (message.token.as_deref() == Some(client_token)).then(|| receipt_for(message, thread))
    })
}

fn scan_folder(dir: &Path, new_folder: bool, sent: bool, out: &mut BTreeMap<String, RawMessage>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    // A stable order makes duplicate ids deterministic even before the
    // mailbox is deduplicated.
    paths.sort();

    for path in paths {
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        // Never follow links: a maildir must not expose arbitrary files.
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            continue;
        }
        if metadata.len() > MAX_MESSAGE_BYTES {
            // Oversized mail is ignored, never an inbox-wide failure.
            continue;
        }

        let unread = if sent {
            false
        } else if new_folder {
            true
        } else {
            !has_seen_flag(&path)
        };

        if let Some(message) = parse_message_file(&path, &metadata, unread) {
            out.entry(message.id.clone()).or_insert(message);
        }
    }
}

fn parse_message_file(path: &Path, metadata: &fs::Metadata, unread: bool) -> Option<RawMessage> {
    let raw = fs::read(path).ok()?;
    let message = MessageParser::new().parse(&raw)?;

    // Arbitrary text parses as a header-less message; require at least one
    // known header before treating a file as mail.
    if !message.headers().has_known() {
        return None;
    }

    let fallback_id = format!("sha256:{}", sha256_hex(&raw));
    let id = message
        .message_id()
        .and_then(normalize_message_id)
        .unwrap_or(fallback_id);

    let (sender, sender_address) = sender_of(&message);
    let subject = message
        .subject()
        .map(str::trim)
        .filter(|subject| !subject.is_empty())
        .map_or_else(|| "(no subject)".to_string(), ToString::to_string);

    let body_text = message
        .body_text(0)
        .map_or_else(String::new, std::borrow::Cow::into_owned);

    let timestamp_ms = message
        .date()
        .map(|date| date.to_timestamp().saturating_mul(1000))
        .or_else(|| file_mtime_ms(metadata))
        .unwrap_or(0);

    let mut references: Vec<String> = Vec::new();
    if let Some(list) = message.references() {
        for reference in list.iter() {
            if let Some(reference) = normalize_message_id(reference) {
                push_unique(&mut references, reference);
            }
        }
    }
    if let Some(list) = message.in_reply_to() {
        for reference in list.iter() {
            if let Some(reference) = normalize_message_id(reference) {
                push_unique(&mut references, reference);
            }
        }
    }

    let token = message
        .headers()
        .get("X-Herald-Token")
        .and_then(|header| header.value().as_text())
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(ToString::to_string);

    Some(RawMessage {
        thread_id: id.clone(),
        id,
        sender,
        sender_address,
        subject,
        body_text,
        timestamp_ms,
        unread,
        references,
        token,
    })
}

fn file_mtime_ms(metadata: &fs::Metadata) -> Option<i64> {
    metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
}

fn has_seen_flag(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.rsplit_once(":2,").or_else(|| name.rsplit_once(";2,")))
        .is_some_and(|(_, flags)| flags.contains('S'))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("{digest:x}")
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
}

/// Union-find over message ids so a thread id does not depend on scan order.
#[derive(Default)]
struct UnionFind {
    parent: Vec<usize>,
    size: Vec<usize>,
    index: HashMap<String, usize>,
}

impl UnionFind {
    fn ensure(&mut self, id: &str) -> usize {
        if let Some(&index) = self.index.get(id) {
            return index;
        }
        let index = self.parent.len();
        self.parent.push(index);
        self.size.push(1);
        self.index.insert(id.to_string(), index);
        index
    }

    fn find(&mut self, id: &str) -> usize {
        let index = self.ensure(id);
        self.find_index(index)
    }

    fn find_index(&mut self, index: usize) -> usize {
        let mut root = index;
        while self.parent[root] != root {
            root = self.parent[root];
        }
        let mut current = index;
        while self.parent[current] != root {
            let next = self.parent[current];
            self.parent[current] = root;
            current = next;
        }
        root
    }

    fn union(&mut self, left: &str, right: &str) {
        let mut left_root = self.find(left);
        let mut right_root = self.find(right);
        if left_root == right_root {
            return;
        }
        if self.size[left_root] < self.size[right_root] {
            std::mem::swap(&mut left_root, &mut right_root);
        }
        self.parent[right_root] = left_root;
        self.size[left_root] += self.size[right_root];
    }
}

/// Assign every message the lexicographically smallest message id in its
/// connected component. Reference ids that are not present in the mailbox are
/// still nodes, so the id is stable when the root message arrives later.
fn assign_thread_ids(messages: &mut [RawMessage]) {
    let mut union_find = UnionFind::default();
    for message in messages.iter() {
        union_find.ensure(&message.id);
        for reference in &message.references {
            union_find.union(&message.id, reference);
        }
    }

    let mut thread_by_root: HashMap<usize, String> = HashMap::new();
    // Include reference-only nodes: a reply whose root is not in the mailbox
    // already belongs to the root's thread, and its id does not change when the
    // root later shows up.
    let nodes: Vec<(String, usize)> = union_find
        .index
        .iter()
        .map(|(id, index)| (id.clone(), *index))
        .collect();
    for (id, index) in nodes {
        let root = union_find.find_index(index);
        thread_by_root
            .entry(root)
            .and_modify(|current| {
                if id < *current {
                    current.clone_from(&id);
                }
            })
            .or_insert(id);
    }

    for message in messages.iter_mut() {
        let root = union_find.find(&message.id);
        message.thread_id = thread_by_root
            .get(&root)
            .cloned()
            .unwrap_or_else(|| message.id.clone());
    }
}

fn choose_recipient(messages: &[RawMessage], from_address: &str) -> Option<String> {
    messages
        .iter()
        .rev()
        .find(|message| {
            message
                .sender_address
                .as_deref()
                .is_some_and(|address| !address.eq_ignore_ascii_case(from_address))
        })
        .or_else(|| messages.last())
        .and_then(|message| message.sender_address.clone())
}

fn extract_address(value: &str) -> Option<String> {
    let value = value.trim();
    let address = if let (Some(start), Some(end)) = (value.find('<'), value.rfind('>')) {
        if end > start {
            value[start + 1..end].trim()
        } else {
            value
        }
    } else {
        value
    };
    if address.is_empty() || address.chars().any(char::is_whitespace) {
        None
    } else {
        Some(address.to_string())
    }
}

fn has_line_break(value: &str) -> bool {
    value.chars().any(|c| c == '\r' || c == '\n')
}

/// Strip CR/LF from a value that will be written into a header. Folding is
/// never reconstructed, so a subject cannot introduce another header.
fn sanitize_header_value(value: &str) -> String {
    value
        .chars()
        .map(|c| if c == '\r' || c == '\n' { ' ' } else { c })
        .collect()
}

fn reply_subject(subject: &str) -> String {
    let trimmed = subject.trim();
    if trimmed
        .get(..3)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("re:"))
    {
        trimmed.to_string()
    } else {
        format!("Re: {trimmed}")
    }
}

fn outbound_message_id(client_token: &str) -> String {
    let mut token: String = client_token
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '@') {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect();
    if token.is_empty() {
        token.push_str("token");
    }
    format!("herald.{token}@herald.local")
}

fn sanitize_id_for_header(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control() && !c.is_whitespace() && *c != '<' && *c != '>')
        .collect()
}

fn normalize_newlines(body: &str) -> String {
    body.replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', "\r\n")
}

struct OutboundMessage<'a> {
    from_header: &'a str,
    recipient: &'a str,
    subject: &'a str,
    in_reply_to: Option<&'a str>,
    references: &'a [String],
    message_id: &'a str,
    client_token: &'a str,
    body: &'a str,
    sent_at_ms: i64,
}

fn build_raw_message(message: &OutboundMessage<'_>) -> Result<Vec<u8>, ProviderError> {
    if has_line_break(message.from_header)
        || has_line_break(message.recipient)
        || has_line_break(message.subject)
    {
        return Err(ProviderError::Other(
            "refusing to write a header containing a line break".into(),
        ));
    }

    let mut raw = String::new();
    raw.push_str("From: ");
    raw.push_str(message.from_header);
    raw.push_str("\r\n");
    raw.push_str("To: ");
    raw.push_str(message.recipient);
    raw.push_str("\r\n");
    raw.push_str("Subject: ");
    raw.push_str(message.subject);
    raw.push_str("\r\n");
    raw.push_str("Date: ");
    raw.push_str(&format_rfc2822_utc(message.sent_at_ms.div_euclid(1000)));
    raw.push_str("\r\n");
    raw.push_str("Message-ID: <");
    raw.push_str(message.message_id);
    raw.push_str(">\r\n");

    if let Some(parent) = message.in_reply_to {
        let parent = sanitize_id_for_header(parent);
        if !parent.is_empty() {
            raw.push_str("In-Reply-To: <");
            raw.push_str(&parent);
            raw.push_str(">\r\n");
        }
    }

    let references: Vec<String> = message
        .references
        .iter()
        .map(|reference| sanitize_id_for_header(reference))
        .filter(|reference| !reference.is_empty())
        .collect();
    if !references.is_empty() {
        raw.push_str("References: ");
        for (index, reference) in references.iter().enumerate() {
            if index > 0 {
                raw.push(' ');
            }
            raw.push('<');
            raw.push_str(reference);
            raw.push('>');
        }
        raw.push_str("\r\n");
    }

    raw.push_str("X-Herald-Token: ");
    raw.push_str(message.client_token);
    raw.push_str("\r\n");
    raw.push_str("MIME-Version: 1.0\r\n");
    raw.push_str("Content-Type: text/plain; charset=utf-8\r\n");
    raw.push_str("Content-Transfer-Encoding: 8bit\r\n");
    raw.push_str("\r\n");
    raw.push_str(&normalize_newlines(message.body));

    Ok(raw.into_bytes())
}

fn verify_sent_file(path: &Path, client_token: &str, body: &str) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    let Some(message) = parse_message_file(path, &metadata, false) else {
        return false;
    };
    if message.token.as_deref() != Some(client_token) {
        return false;
    }
    body_hash(&herald_core::canonical_body_text(&message.body_text))
        == body_hash(&herald_core::canonical_body_text(body))
}

fn receipt_for(message: RawMessage, thread: &ThreadId) -> ExecutionReceipt {
    let sent_at_ms = if message.timestamp_ms > 0 {
        message.timestamp_ms
    } else {
        now_ms()
    };
    ExecutionReceipt {
        provider: ProviderId::Maildir,
        thread_id: thread.clone(),
        provider_message_id: MessageId::new(message.id),
        body_hash: body_hash(&herald_core::canonical_body_text(&message.body_text)),
        sent_at_ms,
        verified: true,
    }
}

fn unique_name() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    format!("{nanos}.{}.{counter}.herald", std::process::id())
}

/// Format a unix timestamp in seconds as an RFC 2822 UTC date.
///
/// Kept dependency-free (no `chrono`) and tested against known dates.
#[must_use]
pub fn format_rfc2822_utc(unix_seconds: i64) -> String {
    const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];

    let days = unix_seconds.div_euclid(86_400);
    let seconds_of_day = unix_seconds.rem_euclid(86_400);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    let weekday = usize::try_from((days + 4).rem_euclid(7)).unwrap_or(0);
    let (year, month, day) = civil_from_days(days);
    let month_name = MONTHS
        .get(usize::try_from(month.saturating_sub(1)).unwrap_or(0))
        .copied()
        .unwrap_or("Jan");

    format!(
        "{}, {day:02} {month_name} {year:04} {hour:02}:{minute:02}:{second:02} +0000",
        WEEKDAYS.get(weekday).copied().unwrap_or("Thu")
    )
}

/// Howard Hinnant's `civil_from_days`: days since 1970-01-01 to a Gregorian
/// date.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::too_many_lines)]

    use super::*;
    use std::sync::Mutex;

    const DATE: &str = "Mon, 01 Jan 2024 10:00:00 +0000";
    const DATE_MS: i64 = 1_704_103_200_000;

    #[derive(Debug, Clone)]
    struct Delivery {
        from: String,
        to: String,
        raw: Vec<u8>,
    }

    #[derive(Debug, Default)]
    struct RecordingTransport {
        calls: Mutex<Vec<Delivery>>,
        fail_with: Mutex<Option<ProviderError>>,
    }

    impl RecordingTransport {
        fn failing(error: ProviderError) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                fail_with: Mutex::new(Some(error)),
            }
        }

        fn calls(&self) -> Vec<Delivery> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl MailTransport for RecordingTransport {
        fn deliver(&self, from: &str, to: &str, raw: &[u8]) -> Result<(), ProviderError> {
            if let Some(error) = self.fail_with.lock().unwrap().clone() {
                return Err(error);
            }
            self.calls.lock().unwrap().push(Delivery {
                from: from.to_string(),
                to: to.to_string(),
                raw: raw.to_vec(),
            });
            Ok(())
        }
    }

    struct Fixture {
        dir: tempfile::TempDir,
        root: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().join("Maildir");
            for folder in ["cur", "new", "tmp"] {
                fs::create_dir_all(root.join(folder)).unwrap();
            }
            Self { dir, root }
        }

        fn config(&self) -> MaildirConfig {
            MaildirConfig {
                root: self.root.clone(),
                sent_path: None,
                from: Some("Me <me@example.com>".into()),
                smtp: Some(smtp()),
            }
        }

        fn read_only_config(&self) -> MaildirConfig {
            MaildirConfig {
                smtp: None,
                ..self.config()
            }
        }

        fn write(&self, folder: &str, name: &str, raw: &str) -> PathBuf {
            let path = self.root.join(folder).join(name);
            fs::write(&path, raw).unwrap();
            path
        }

        fn write_new(&self, name: &str, raw: &str) -> PathBuf {
            self.write("new", name, raw)
        }

        fn write_cur(&self, name: &str, flags: &str, raw: &str) -> PathBuf {
            self.write("cur", &format!("{name}{FLAG_SEPARATOR}2,{flags}"), raw)
        }
    }

    fn smtp() -> SmtpConfig {
        SmtpConfig {
            host: "localhost".into(),
            port: 25,
            username: None,
            password: None,
            security: SmtpSecurity::None,
        }
    }

    fn raw_message(
        from: &str,
        subject: &str,
        message_id: Option<&str>,
        references: &[&str],
        date: &str,
        body: &str,
    ) -> String {
        let mut raw =
            format!("From: {from}\r\nTo: me@example.com\r\nSubject: {subject}\r\nDate: {date}\r\n");
        if let Some(message_id) = message_id {
            raw.push_str("Message-ID: <");
            raw.push_str(message_id);
            raw.push_str(">\r\n");
        }
        if !references.is_empty() {
            raw.push_str("References: ");
            for (index, reference) in references.iter().enumerate() {
                if index > 0 {
                    raw.push(' ');
                }
                raw.push('<');
                raw.push_str(reference);
                raw.push('>');
            }
            raw.push_str("\r\n");
        }
        raw.push_str("MIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n");
        raw.push_str(body);
        raw
    }

    fn provider_for(fixture: &Fixture) -> MaildirProvider {
        MaildirProvider::new(fixture.config())
    }

    #[tokio::test]
    async fn unread_semantics_follow_maildir_flags() {
        let fixture = Fixture::new();
        fixture.write_new(
            "a",
            &raw_message("A <a@example.com>", "new", Some("a@x"), &[], DATE, "one"),
        );
        fixture.write_cur(
            "b",
            "S",
            &raw_message("B <b@example.com>", "seen", Some("b@x"), &[], DATE, "two"),
        );
        fixture.write_cur(
            "c",
            "",
            &raw_message(
                "C <c@example.com>",
                "unseen",
                Some("c@x"),
                &[],
                DATE,
                "three",
            ),
        );
        fixture.write_cur(
            "d",
            "FS",
            &raw_message(
                "D <d@example.com>",
                "flagged",
                Some("d@x"),
                &[],
                DATE,
                "four",
            ),
        );

        let provider = provider_for(&fixture);
        let messages = provider
            .list_recent(&MessageQuery::recent(50))
            .await
            .unwrap();
        let unread: Vec<&str> = messages
            .iter()
            .filter(|message| message.unread)
            .map(|message| message.id.as_str())
            .collect();

        assert_eq!(unread.len(), 2);
        assert!(unread.contains(&"a@x"));
        assert!(unread.contains(&"c@x"));
    }

    #[tokio::test]
    async fn list_recent_sorts_newest_first_and_honours_limit() {
        let fixture = Fixture::new();
        fixture.write_new(
            "a",
            &raw_message(
                "A <a@example.com>",
                "oldest",
                Some("a@x"),
                &[],
                "Mon, 01 Jan 2024 09:00:00 +0000",
                "one",
            ),
        );
        fixture.write_new(
            "b",
            &raw_message(
                "B <b@example.com>",
                "newest",
                Some("b@x"),
                &[],
                "Mon, 01 Jan 2024 11:00:00 +0000",
                "two",
            ),
        );
        fixture.write_new(
            "c",
            &raw_message(
                "C <c@example.com>",
                "middle",
                Some("c@x"),
                &[],
                "Mon, 01 Jan 2024 10:00:00 +0000",
                "three",
            ),
        );

        let provider = provider_for(&fixture);
        let messages = provider
            .list_recent(&MessageQuery::recent(2))
            .await
            .unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].subject, "newest");
        assert_eq!(messages[1].subject, "middle");

        let default_limit = provider
            .list_recent(&MessageQuery::default())
            .await
            .unwrap();
        assert_eq!(default_limit.len(), 3);
    }

    #[tokio::test]
    async fn list_recent_filters_unread() {
        let fixture = Fixture::new();
        fixture.write_new(
            "a",
            &raw_message("A <a@example.com>", "new", Some("a@x"), &[], DATE, "one"),
        );
        fixture.write_cur(
            "b",
            "S",
            &raw_message("B <b@example.com>", "seen", Some("b@x"), &[], DATE, "two"),
        );

        let provider = provider_for(&fixture);
        let unread = provider
            .list_recent(&MessageQuery::unread(10))
            .await
            .unwrap();
        assert_eq!(unread.len(), 1);
        assert_eq!(unread[0].id.as_str(), "a@x");
    }

    #[tokio::test]
    async fn search_is_case_insensitive_across_fields() {
        let fixture = Fixture::new();
        fixture.write_new(
            "a",
            &raw_message(
                "Alex Founder <alex@example.com>",
                "Budget review",
                Some("a@x"),
                &[],
                DATE,
                "The numbers look good.",
            ),
        );

        let provider = provider_for(&fixture);
        for needle in ["BUDGET", "ALEX", "NuMbErS"] {
            let query = MessageQuery {
                search: Some(needle.to_string()),
                ..MessageQuery::recent(10)
            };
            let found = provider.list_recent(&query).await.unwrap();
            assert_eq!(found.len(), 1, "search for {needle}");
        }

        let query = MessageQuery {
            search: Some("absent".into()),
            ..MessageQuery::recent(10)
        };
        assert_eq!(provider.list_recent(&query).await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn replies_are_grouped_under_the_root_id() {
        let fixture = Fixture::new();
        fixture.write_new(
            "a",
            &raw_message(
                "Root <root@example.com>",
                "Plan",
                Some("a-root@x"),
                &[],
                "Mon, 01 Jan 2024 09:00:00 +0000",
                "root",
            ),
        );
        fixture.write_cur(
            "b",
            "S",
            &raw_message(
                "Reply <r1@example.com>",
                "Re: Plan",
                Some("b-r1@x"),
                &["a-root@x"],
                "Mon, 01 Jan 2024 10:00:00 +0000",
                "one",
            ),
        );
        fixture.write_new(
            "c",
            &raw_message(
                "Reply <r2@example.com>",
                "Re: Plan",
                Some("c-r2@x"),
                &["a-root@x", "b-r1@x"],
                "Mon, 01 Jan 2024 11:00:00 +0000",
                "two",
            ),
        );

        let provider = provider_for(&fixture);
        let thread = provider
            .get_thread(&ThreadId::new("a-root@x"))
            .await
            .unwrap();
        assert_eq!(thread.id.as_str(), "a-root@x");
        assert_eq!(thread.subject, "Plan");
        assert_eq!(thread.messages.len(), 3);
        assert_eq!(thread.messages[0].id.as_str(), "a-root@x");
        assert_eq!(thread.messages[2].id.as_str(), "c-r2@x");
        assert!(
            thread
                .messages
                .iter()
                .all(|message| message.thread_id.as_str() == "a-root@x")
        );
    }

    #[tokio::test]
    async fn a_reply_whose_root_is_absent_uses_the_referenced_id() {
        let fixture = Fixture::new();
        fixture.write_new(
            "a",
            &raw_message(
                "Reply <reply@example.com>",
                "Re: Plan",
                Some("zz-reply@x"),
                &["aa-root@x"],
                DATE,
                "orphan",
            ),
        );

        let provider = provider_for(&fixture);
        let thread = provider
            .get_thread(&ThreadId::new("aa-root@x"))
            .await
            .unwrap();
        assert_eq!(thread.id.as_str(), "aa-root@x");
        assert_eq!(thread.messages.len(), 1);
        assert_eq!(thread.messages[0].thread_id.as_str(), "aa-root@x");
    }

    #[tokio::test]
    async fn thread_id_is_stable_when_files_are_scanned_in_another_order() {
        let forward = Fixture::new();
        let reverse = Fixture::new();
        let messages = [
            raw_message(
                "Root <root@example.com>",
                "Plan",
                Some("a-root@x"),
                &[],
                DATE,
                "root",
            ),
            raw_message(
                "Reply <r1@example.com>",
                "Re: Plan",
                Some("b-r1@x"),
                &["a-root@x"],
                DATE,
                "one",
            ),
            raw_message(
                "Reply <r2@example.com>",
                "Re: Plan",
                Some("c-r2@x"),
                &["a-root@x", "b-r1@x"],
                DATE,
                "two",
            ),
        ];
        for (index, message) in messages.iter().enumerate() {
            forward.write_new(&format!("{index}"), message);
            reverse.write_new(&format!("{}", messages.len() - index), message);
        }

        let a = provider_for(&forward);
        let b = provider_for(&reverse);
        let a_messages = a.list_recent(&MessageQuery::recent(10)).await.unwrap();
        let b_messages = b.list_recent(&MessageQuery::recent(10)).await.unwrap();
        let mut a_threads: Vec<String> = a_messages.iter().map(|m| m.thread_id.0.clone()).collect();
        let mut b_threads: Vec<String> = b_messages.iter().map(|m| m.thread_id.0.clone()).collect();
        a_threads.sort();
        b_threads.sort();
        assert_eq!(a_threads, vec!["a-root@x"; 3]);
        assert_eq!(b_threads, a_threads);
    }

    #[tokio::test]
    async fn unknown_thread_is_not_found() {
        let fixture = Fixture::new();
        let provider = provider_for(&fixture);
        assert_eq!(
            provider
                .get_thread(&ThreadId::new("missing@x"))
                .await
                .unwrap_err(),
            ProviderError::NotFound
        );
    }

    #[tokio::test]
    async fn sender_subject_and_date_are_parsed() {
        let fixture = Fixture::new();
        fixture.write_new(
            "a",
            &raw_message(
                "Alex Founder <alex@example.com>",
                "   ",
                Some("a@x"),
                &[],
                DATE,
                "hello",
            ),
        );
        let provider = provider_for(&fixture);
        let messages = provider
            .list_recent(&MessageQuery::recent(10))
            .await
            .unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].sender, "Alex Founder <alex@example.com>");
        assert_eq!(messages[0].subject, "(no subject)");
        assert_eq!(messages[0].timestamp_ms, DATE_MS);
    }

    #[tokio::test]
    async fn missing_message_id_falls_back_to_a_stable_hash() {
        let fixture = Fixture::new();
        fixture.write_new(
            "a",
            &raw_message("A <a@example.com>", "no id", None, &[], DATE, "hello"),
        );
        let provider = provider_for(&fixture);

        let first = provider
            .list_recent(&MessageQuery::recent(10))
            .await
            .unwrap();
        let second = provider
            .list_recent(&MessageQuery::recent(10))
            .await
            .unwrap();
        assert_eq!(first.len(), 1);
        assert!(first[0].id.as_str().starts_with("sha256:"));
        assert_eq!(first[0].id, second[0].id);
    }

    #[tokio::test]
    async fn malformed_file_is_skipped_not_fatal() {
        let fixture = Fixture::new();
        fixture.write_new("a", "this is not a message at all\n");
        fixture.write_new(
            "b",
            &raw_message("A <a@example.com>", "good", Some("a@x"), &[], DATE, "hello"),
        );
        let provider = provider_for(&fixture);
        let messages = provider
            .list_recent(&MessageQuery::recent(10))
            .await
            .unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].id.as_str(), "a@x");
    }

    #[tokio::test]
    async fn oversized_file_is_skipped() {
        let fixture = Fixture::new();
        let oversized = vec![b'x'; usize::try_from(MAX_MESSAGE_BYTES).unwrap() + 1];
        fs::write(fixture.root.join("new").join("huge"), &oversized).unwrap();
        fixture.write_new(
            "b",
            &raw_message("A <a@example.com>", "good", Some("a@x"), &[], DATE, "hello"),
        );
        let provider = provider_for(&fixture);
        let messages = provider
            .list_recent(&MessageQuery::recent(10))
            .await
            .unwrap();
        assert_eq!(messages.len(), 1);
    }

    #[tokio::test]
    async fn health_reports_ready_and_unavailable() {
        let fixture = Fixture::new();
        let provider = provider_for(&fixture);
        assert_eq!(provider.health().await, ProviderHealth::Ready);

        let missing = MaildirProvider::new(MaildirConfig {
            root: fixture.dir.path().join("nope"),
            sent_path: None,
            from: None,
            smtp: None,
        });
        assert!(matches!(
            missing.health().await,
            ProviderHealth::Unavailable(_)
        ));
    }

    #[tokio::test]
    async fn prepare_reply_rejects_empty_body_and_unknown_thread() {
        let fixture = Fixture::new();
        fixture.write_new(
            "a",
            &raw_message("A <a@example.com>", "hi", Some("a@x"), &[], DATE, "hello"),
        );
        let provider = provider_for(&fixture);

        assert_eq!(
            provider
                .prepare_reply(&ThreadId::new("a@x"), "   ", "token-1")
                .await
                .unwrap_err(),
            ProviderError::Other("empty reply body".into())
        );
        assert_eq!(
            provider
                .prepare_reply(&ThreadId::new("missing@x"), "hello", "token-1")
                .await
                .unwrap_err(),
            ProviderError::NotFound
        );
    }

    fn thread_fixture() -> Fixture {
        let fixture = Fixture::new();
        fixture.write_new(
            "a",
            &raw_message(
                "Alex <alex@example.com>",
                "Plan",
                Some("a-root@x"),
                &[],
                "Mon, 01 Jan 2024 08:00:00 +0000",
                "root",
            ),
        );
        fixture.write_new(
            "b",
            &raw_message(
                "Me <me@example.com>",
                "Re: Plan",
                Some("b-me@x"),
                &["a-root@x"],
                "Mon, 01 Jan 2024 09:00:00 +0000",
                "my last mail",
            ),
        );
        fixture.write_new(
            "c",
            &raw_message(
                "Alex <alex@example.com>",
                "Re: Plan",
                Some("c-alex2@x"),
                &["a-root@x", "b-me@x"],
                "Mon, 01 Jan 2024 10:00:00 +0000",
                "latest",
            ),
        );
        fixture
    }

    #[tokio::test]
    async fn send_delivers_appends_and_verifies() {
        let fixture = thread_fixture();
        let transport = Arc::new(RecordingTransport::default());
        let provider = MaildirProvider::with_transport(fixture.config(), transport.clone());
        let thread_id = ThreadId::new("a-root@x");

        let prepared = provider
            .prepare_reply(&thread_id, "Thanks for the update.", "token-1")
            .await
            .unwrap();
        let receipt = provider.send_reply(&prepared).await.unwrap();
        assert!(receipt.verified);
        assert_eq!(
            receipt.provider_message_id.as_str(),
            "herald.token-1@herald.local"
        );

        let calls = transport.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].from, "me@example.com");
        assert_eq!(calls[0].to, "alex@example.com");

        let raw = String::from_utf8(calls[0].raw.clone()).unwrap();
        assert!(raw.contains("From: Me <me@example.com>\r\n"));
        assert!(raw.contains("To: alex@example.com\r\n"));
        assert!(raw.contains("Subject: Re: Plan\r\n"));
        assert!(raw.contains("In-Reply-To: <c-alex2@x>\r\n"));
        assert!(raw.contains("References: <a-root@x> <b-me@x> <c-alex2@x>\r\n"));
        assert!(raw.contains("X-Herald-Token: token-1\r\n"));
        assert_eq!(raw.matches("Subject:").count(), 1);

        let sent_file = fixture.root.join(".Sent").join("cur");
        let entries: Vec<_> = fs::read_dir(&sent_file)
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(entries.len(), 1);
        assert!(
            entries[0]
                .file_name()
                .to_string_lossy()
                .ends_with(&format!("{FLAG_SEPARATOR}2,S"))
        );
        // The Sent copy is the exact bytes that were handed to SMTP.
        assert_eq!(fs::read(entries[0].path()).unwrap(), calls[0].raw);

        // A retry with the same token is idempotent: no second delivery. The
        // recovered receipt is rebuilt from the Sent copy, so the whole-second
        // Date header makes `sent_at_ms` coarser; the identity fields match.
        let again = provider.send_reply(&prepared).await.unwrap();
        assert!(again.verified);
        assert_eq!(again.thread_id, receipt.thread_id);
        assert_eq!(again.provider_message_id, receipt.provider_message_id);
        assert_eq!(again.body_hash, receipt.body_hash);
        assert_eq!(transport.calls().len(), 1);
    }

    #[tokio::test]
    async fn send_without_smtp_is_not_configured_and_leaves_mailbox_untouched() {
        let fixture = thread_fixture();
        let transport = Arc::new(RecordingTransport::default());
        let provider =
            MaildirProvider::with_transport(fixture.read_only_config(), transport.clone());
        let prepared = provider
            .prepare_reply(&ThreadId::new("a-root@x"), "hello", "token-1")
            .await
            .unwrap();

        assert!(matches!(
            provider.send_reply(&prepared).await.unwrap_err(),
            ProviderError::NotConfigured(_)
        ));
        assert!(transport.calls().is_empty());
        assert!(!fixture.root.join(".Sent").exists());
    }

    #[tokio::test]
    async fn send_without_from_is_not_configured() {
        let fixture = thread_fixture();
        let transport = Arc::new(RecordingTransport::default());
        let config = MaildirConfig {
            from: None,
            ..fixture.config()
        };
        let provider = MaildirProvider::with_transport(config, transport.clone());
        let prepared = provider
            .prepare_reply(&ThreadId::new("a-root@x"), "hello", "token-1")
            .await
            .unwrap();

        assert!(matches!(
            provider.send_reply(&prepared).await.unwrap_err(),
            ProviderError::NotConfigured(_)
        ));
        assert!(transport.calls().is_empty());
    }

    #[tokio::test]
    async fn transport_timeout_is_reported_and_leaves_no_sent_copy() {
        let fixture = thread_fixture();
        let transport = Arc::new(RecordingTransport::failing(ProviderError::Timeout));
        let provider = MaildirProvider::with_transport(fixture.config(), transport.clone());
        let prepared = provider
            .prepare_reply(&ThreadId::new("a-root@x"), "hello", "token-1")
            .await
            .unwrap();

        assert_eq!(
            provider.send_reply(&prepared).await.unwrap_err(),
            ProviderError::Timeout
        );
        assert!(!fixture.root.join(".Sent").exists());
    }

    #[tokio::test]
    async fn sent_append_failure_still_returns_an_unverified_receipt() {
        let fixture = thread_fixture();
        let transport = Arc::new(RecordingTransport::default());
        let blocker = fixture.dir.path().join("sent-is-a-file");
        fs::write(&blocker, b"not a maildir").unwrap();
        let config = MaildirConfig {
            sent_path: Some(blocker),
            ..fixture.config()
        };
        let provider = MaildirProvider::with_transport(config, transport.clone());
        let prepared = provider
            .prepare_reply(&ThreadId::new("a-root@x"), "hello", "token-1")
            .await
            .unwrap();

        let receipt = provider.send_reply(&prepared).await.unwrap();
        assert!(!receipt.verified);
        assert_eq!(transport.calls().len(), 1);
    }

    #[tokio::test]
    async fn find_sent_finds_the_appended_message() {
        let fixture = thread_fixture();
        let transport = Arc::new(RecordingTransport::default());
        let provider = MaildirProvider::with_transport(fixture.config(), transport.clone());
        let thread_id = ThreadId::new("a-root@x");
        let body = "Thanks for the update.";
        let prepared = provider
            .prepare_reply(&thread_id, body, "token-1")
            .await
            .unwrap();

        assert!(
            provider
                .find_sent(&thread_id, "token-1")
                .await
                .unwrap()
                .is_none()
        );

        let receipt = provider.send_reply(&prepared).await.unwrap();
        let found = provider
            .find_sent(&thread_id, "token-1")
            .await
            .unwrap()
            .expect("sent copy");
        assert!(found.verified);
        assert_eq!(found.provider_message_id, receipt.provider_message_id);
        assert_eq!(found.body_hash, body_hash(body));
        assert!(
            provider
                .find_sent(&thread_id, "other-token")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn header_injection_in_from_is_rejected() {
        let fixture = thread_fixture();
        let transport = Arc::new(RecordingTransport::default());
        let config = MaildirConfig {
            from: Some("Me <me@example.com>\r\nBcc: evil@example.com".into()),
            ..fixture.config()
        };
        let provider = MaildirProvider::with_transport(config, transport.clone());
        let prepared = provider
            .prepare_reply(&ThreadId::new("a-root@x"), "hello", "token-1")
            .await
            .unwrap();

        assert!(provider.send_reply(&prepared).await.is_err());
        assert!(transport.calls().is_empty());
    }

    #[tokio::test]
    async fn subject_line_breaks_cannot_add_headers() {
        let fixture = Fixture::new();
        // A folded header is unfolded by the parser, but if any newline survives
        // it must be flattened before the reply is written.
        fixture.write_new(
            "a",
            "From: Alex <alex@example.com>\r\nSubject: hello\r\n Bcc: evil@example.com\r\nDate: Mon, 01 Jan 2024 10:00:00 +0000\r\nMessage-ID: <a@x>\r\n\r\nbody",
        );
        let transport = Arc::new(RecordingTransport::default());
        let provider = MaildirProvider::with_transport(fixture.config(), transport.clone());
        let prepared = provider
            .prepare_reply(&ThreadId::new("a@x"), "hi", "token-1")
            .await
            .unwrap();
        provider.send_reply(&prepared).await.unwrap();

        let raw = String::from_utf8(transport.calls()[0].raw.clone()).unwrap();
        assert_eq!(raw.matches("Subject:").count(), 1);
        assert_eq!(
            raw.lines().filter(|line| line.starts_with("Bcc:")).count(),
            0
        );
    }

    #[test]
    fn smtp_debug_redacts_the_password() {
        let config = SmtpConfig {
            host: "smtp.example.com".into(),
            port: 587,
            username: Some("me@example.com".into()),
            password: Some("hunter2".into()),
            security: SmtpSecurity::Starttls,
        };
        let debug = format!("{config:?}");
        assert!(!debug.contains("hunter2"));
        assert!(debug.contains("redacted"));
    }

    #[test]
    fn rfc2822_dates_are_formatted_in_utc() {
        assert_eq!(format_rfc2822_utc(0), "Thu, 01 Jan 1970 00:00:00 +0000");
        assert_eq!(
            format_rfc2822_utc(1_704_103_200),
            "Mon, 01 Jan 2024 10:00:00 +0000"
        );
        assert_eq!(
            format_rfc2822_utc(1_700_000_000),
            "Tue, 14 Nov 2023 22:13:20 +0000"
        );
    }

    #[test]
    fn subject_reply_prefix_is_not_doubled() {
        assert_eq!(reply_subject("Plan"), "Re: Plan");
        assert_eq!(reply_subject("re: Plan"), "re: Plan");
        assert_eq!(reply_subject("RE: Plan"), "RE: Plan");
    }

    #[test]
    fn header_sanitizer_flattens_line_breaks() {
        assert_eq!(sanitize_header_value("hi\r\nBcc: x"), "hi  Bcc: x");
    }

    #[test]
    fn symlinked_message_files_are_ignored() {
        let fixture = Fixture::new();
        let outside = fixture.dir.path().join("outside.eml");
        fs::write(
            &outside,
            raw_message(
                "A <a@example.com>",
                "linked",
                Some("a@x"),
                &[],
                DATE,
                "hello",
            ),
        )
        .unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, fixture.root.join("new").join("link")).unwrap();

        let messages = scan_inbox(&fixture.root);
        assert!(messages.is_empty());
    }
}
