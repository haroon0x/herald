//! One clock for the whole system: milliseconds since the Unix epoch, UTC.
//! Everything compares against this and nothing else.

use std::time::{SystemTime, UNIX_EPOCH};

/// Current unix time in milliseconds.
#[must_use]
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}
