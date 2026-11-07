//! RFC 822 header parsing shared by mail-format providers (`maildir`, `imap`).
//!
//! Message-id normalization and From formatting are provider-neutral: Gmail's
//! IMAP and a local Maildir face the same RFC. Provider-specific quirks stay
//! in their own modules.

/// Normalize a `Message-ID` header value: strip one pair of angle brackets,
/// reject whitespace and control characters.
pub(crate) fn normalize_message_id(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let stripped = trimmed.trim_start_matches('<').trim_end_matches('>').trim();
    if stripped.is_empty() || stripped.chars().any(char::is_control) {
        None
    } else {
        Some(stripped.to_string())
    }
}

/// Format a parsed `From` header as `Name <address>` (or whichever parts exist).
pub(crate) fn sender_of(message: &mail_parser::Message<'_>) -> (String, Option<String>) {
    let Some(mailbox) = message.from().and_then(|list| list.first()) else {
        return ("unknown".to_string(), None);
    };

    let name = mailbox
        .name()
        .map(str::trim)
        .filter(|name| !name.is_empty());
    let address = mailbox
        .address()
        .map(str::trim)
        .filter(|address| !address.is_empty());

    let display = match (name, address) {
        (Some(name), Some(address)) => format!("{name} <{address}>"),
        (Some(name), None) => name.to_string(),
        (None, Some(address)) => address.to_string(),
        (None, None) => "unknown".to_string(),
    };
    (display, address.map(ToString::to_string))
}
