//! Provider-facing types: `ProviderId`, message ids, threads, and `Untrusted`.
//! `ProviderId` is a closed enum; adding one is a code change, not a config change.

use serde::{Deserialize, Serialize};
use std::fmt;

/// A communication provider Herald can talk to.
///
/// This is deliberately a closed enum. The planner cannot invent a provider;
/// adding one is a code change that goes through review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderId {
    Gmail,
    X,
    Fake,
    Maildir,
}

impl ProviderId {
    /// Every provider this build knows about, implemented or planned.
    pub const ALL: [Self; 4] = [Self::Fake, Self::Maildir, Self::Gmail, Self::X];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Gmail => "gmail",
            Self::X => "x",
            Self::Fake => "fake",
            Self::Maildir => "maildir",
        }
    }

    /// Whether this build contains an executable adapter for the provider.
    ///
    /// Planned providers still parse and plan, so the CLI can explain that the
    /// plan is inert instead of pretending a read happened.
    #[must_use]
    pub const fn is_implemented(self) -> bool {
        matches!(self, Self::Fake | Self::Maildir | Self::X)
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ProviderId {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "gmail" => Ok(Self::Gmail),
            "x" => Ok(Self::X),
            "fake" | "demo" => Ok(Self::Fake),
            "maildir" => Ok(Self::Maildir),
            other => Err(format!("unknown provider: {other}")),
        }
    }
}

/// Stable provider message identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MessageId(pub String);

impl MessageId {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MessageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A message summary. Bodies are not stored here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageSummary {
    pub id: MessageId,
    pub thread_id: crate::action::ThreadId,
    pub sender: String,
    pub subject: String,
    pub timestamp_ms: i64,
    pub unread: bool,
}

/// A single message inside a thread.
///
/// `body` is untrusted input. It is wrapped in [`Untrusted`] so it cannot be
/// mistaken for instructions when building prompts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderMessage {
    pub id: MessageId,
    pub thread_id: crate::action::ThreadId,
    pub sender: String,
    pub subject: String,
    pub body: Untrusted<String>,
    pub timestamp_ms: i64,
    pub unread: bool,
}

/// Text that came from outside Herald and must be treated as data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Untrusted<T>(pub T);

impl<T> Untrusted<T> {
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T: AsRef<str>> AsRef<str> for Untrusted<T> {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}

/// A full conversation thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thread {
    pub id: crate::action::ThreadId,
    pub provider: ProviderId,
    pub subject: String,
    pub messages: Vec<ProviderMessage>,
}

/// Query used by read-only provider operations.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageQuery {
    pub unread_only: bool,
    pub search: Option<String>,
    pub limit: usize,
}

impl MessageQuery {
    #[must_use]
    pub fn recent(limit: usize) -> Self {
        Self {
            limit,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn unread(limit: usize) -> Self {
        Self {
            unread_only: true,
            limit,
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_id_round_trips_through_json() {
        let parsed: ProviderId = serde_json::from_str("\"gmail\"").unwrap();
        assert_eq!(parsed, ProviderId::Gmail);
        assert_eq!(serde_json::to_string(&parsed).unwrap(), "\"gmail\"");
    }

    #[test]
    fn unknown_provider_is_rejected() {
        assert!("myspace".parse::<ProviderId>().is_err());
    }

    #[test]
    fn demo_is_an_alias_for_the_fake_provider() {
        assert_eq!("demo".parse::<ProviderId>().unwrap(), ProviderId::Fake);
    }

    #[test]
    fn maildir_is_a_real_provider() {
        assert_eq!(
            "maildir".parse::<ProviderId>().unwrap(),
            ProviderId::Maildir
        );
        assert!(ProviderId::Maildir.is_implemented());
    }

    #[test]
    fn planned_providers_are_marked_as_such() {
        assert!(!ProviderId::Gmail.is_implemented());
        assert!(ProviderId::X.is_implemented());
    }
}
