use serde::{Deserialize, Serialize};
use std::fmt;

/// A communication provider Herald can talk to.
///
/// This is deliberately a closed enum. The planner cannot invent a provider;
/// adding one is a code change that goes through review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderId {
    Gmail,
    X,
    Fake,
}

impl ProviderId {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Gmail => "gmail",
            Self::X => "x",
            Self::Fake => "fake",
        }
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
            "fake" => Ok(Self::Fake),
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
}
