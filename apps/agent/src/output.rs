//! Terminal output, color, and the JSON envelope.

use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Value, json};

use crate::cli::Cli;

static JSON_MODE: AtomicBool = AtomicBool::new(false);
static COLOR: AtomicBool = AtomicBool::new(false);

/// Configure the process-wide output mode once, from the global flags.
pub fn init(cli: &Cli) {
    JSON_MODE.store(cli.json, Ordering::Relaxed);
    let no_color_env = std::env::var_os("NO_COLOR").is_some();
    let color = !cli.no_color && !cli.json && !no_color_env && std::io::stdout().is_terminal();
    COLOR.store(color, Ordering::Relaxed);
}

#[must_use]
pub fn json_mode() -> bool {
    JSON_MODE.load(Ordering::Relaxed)
}

#[must_use]
pub fn color_enabled() -> bool {
    COLOR.load(Ordering::Relaxed)
}

/// Human output and JSON envelopes for one command.
#[derive(Debug, Clone, Copy)]
pub struct Out {
    json: bool,
    color: bool,
}

impl Out {
    #[must_use]
    pub fn new() -> Self {
        Self {
            json: json_mode(),
            color: color_enabled(),
        }
    }

    /// Print a human line (no-op in JSON mode).
    pub fn line(self, text: impl AsRef<str>) {
        if !self.json {
            println!("{}", text.as_ref());
        }
    }

    pub fn blank(self) {
        if !self.json {
            println!();
        }
    }

    /// Print an aligned human field, e.g. `decision:  awaiting approval`.
    pub fn field(self, label: &str, value: impl std::fmt::Display) {
        if !self.json {
            println!("{:<11} {value}", format!("{label}:"));
        }
    }

    pub fn heading(self, text: impl AsRef<str>) {
        if !self.json {
            println!("{}", self.bold(text.as_ref()));
        }
    }

    /// Warnings always go to stderr so JSON stdout stays machine-readable.
    #[allow(clippy::unused_self)]
    pub fn warn(self, text: impl AsRef<str>) {
        eprintln!("warning: {}", text.as_ref());
    }

    /// Emit the success envelope (JSON) or nothing (human).
    #[allow(clippy::needless_pass_by_value)]
    pub fn data(self, command: &str, data: Value) {
        if self.json {
            println!(
                "{}",
                json!({
                    "ok": true,
                    "command": command,
                    "data": data,
                })
            );
        }
    }

    #[must_use]
    pub fn bold(self, text: &str) -> String {
        self.paint("1", text)
    }

    #[must_use]
    pub fn amber(self, text: &str) -> String {
        self.paint("38;5;214", text)
    }

    #[must_use]
    pub fn green(self, text: &str) -> String {
        self.paint("32", text)
    }

    #[must_use]
    pub fn red(self, text: &str) -> String {
        self.paint("31", text)
    }

    fn paint(self, code: &str, text: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }
}

impl Default for Out {
    fn default() -> Self {
        Self::new()
    }
}

/// Format a millisecond timestamp as an RFC 3339-ish UTC string without a
/// date-time dependency. Used for human listings only.
#[must_use]
pub fn format_time_ms(ms: i64) -> String {
    let seconds = ms.div_euclid(1000);
    let days = seconds.div_euclid(86_400);
    let rem = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}Z",
        rem / 3_600,
        (rem % 3_600) / 60,
        rem % 60
    )
}

/// Howard Hinnant's civil-from-days algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let month = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_epoch_and_a_known_date() {
        assert_eq!(format_time_ms(0), "1970-01-01 00:00:00Z");
        assert_eq!(format_time_ms(1_700_000_000_000), "2023-11-14 22:13:20Z");
    }

    #[test]
    fn handles_pre_epoch_timestamps_without_panicking() {
        assert_eq!(format_time_ms(-1_000), "1969-12-31 23:59:59Z");
    }
}
