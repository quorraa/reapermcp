//! Diagnostics — stderr only, never stdout.
//!
//! The product's transport invariant is that stdout carries JSON-RPC bytes and
//! nothing else. Everything here therefore writes to stderr, including the
//! panic hook installed by [`install_panic_hook`]: the default Rust hook also
//! writes to stderr, but it is replaced anyway so the message is a single
//! structured line and so a panic can never be mistaken for silence.
//!
//! # Redaction
//!
//! Brief §31 requires that complete user MIDI content stays out of ordinary
//! logs. [`note_summary`] is the only sanctioned way to log anything about a
//! note collection at [`Level::Info`] or above: it reports counts and a span,
//! never pitches. Full note content is reachable only at [`Level::Trace`],
//! which is off unless `QLABS_MCP_LOG=trace` is set explicitly.

use std::io::Write;
use std::sync::atomic::{AtomicU8, Ordering};

/// The environment variable that selects the log level.
pub const LOG_ENV: &str = "QLABS_MCP_LOG";

/// How much to say.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Nothing at all.
    Off,
    /// Failures only.
    Error,
    /// Failures and things that look wrong.
    Warn,
    /// Lifecycle events.
    Info,
    /// Per-request detail.
    Debug,
    /// Everything, including full note content. Never on by default.
    Trace,
}

impl Level {
    /// The stable lowercase identifier.
    pub fn id(self) -> &'static str {
        match self {
            Level::Off => "off",
            Level::Error => "error",
            Level::Warn => "warn",
            Level::Info => "info",
            Level::Debug => "debug",
            Level::Trace => "trace",
        }
    }

    /// Parses an identifier, case-insensitively.
    pub fn parse(s: &str) -> Option<Level> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "silent" => Some(Level::Off),
            "error" => Some(Level::Error),
            "warn" | "warning" => Some(Level::Warn),
            "info" => Some(Level::Info),
            "debug" => Some(Level::Debug),
            "trace" => Some(Level::Trace),
            _ => None,
        }
    }

    fn code(self) -> u8 {
        self as u8
    }

    fn from_code(v: u8) -> Level {
        match v {
            0 => Level::Off,
            1 => Level::Error,
            2 => Level::Warn,
            3 => Level::Info,
            4 => Level::Debug,
            _ => Level::Trace,
        }
    }
}

/// `u8::MAX` means "not yet resolved from the environment".
static LEVEL: AtomicU8 = AtomicU8::new(u8::MAX);

/// The active level, resolved from `QLABS_MCP_LOG` on first use.
pub fn level() -> Level {
    let current = LEVEL.load(Ordering::Relaxed);
    if current != u8::MAX {
        return Level::from_code(current);
    }
    let resolved = std::env::var(LOG_ENV)
        .ok()
        .and_then(|v| Level::parse(&v))
        .unwrap_or(Level::Warn);
    LEVEL.store(resolved.code(), Ordering::Relaxed);
    resolved
}

/// Overrides the level for the rest of the process.
pub fn set_level(l: Level) {
    LEVEL.store(l.code(), Ordering::Relaxed);
}

/// True when a message at `l` would be emitted.
pub fn enabled(l: Level) -> bool {
    l <= level()
}

/// Writes one diagnostic line to stderr.
pub fn log(l: Level, message: &str) {
    if !enabled(l) || l == Level::Off {
        return;
    }
    let line = format!(
        "{} [{}] qlabs-reaper-music-mcp: {}\n",
        qjson::time::now_iso8601(),
        l.id(),
        message
    );
    // stderr write failures are unrecoverable and deliberately ignored: there
    // is nowhere left to report them that is not stdout.
    let _ = std::io::stderr().write_all(line.as_bytes());
}

/// Logs at [`Level::Error`].
pub fn error(message: &str) {
    log(Level::Error, message);
}

/// Logs at [`Level::Warn`].
pub fn warn(message: &str) {
    log(Level::Warn, message);
}

/// Logs at [`Level::Info`].
pub fn info(message: &str) {
    log(Level::Info, message);
}

/// Logs at [`Level::Debug`].
pub fn debug(message: &str) {
    log(Level::Debug, message);
}

/// Logs at [`Level::Trace`]. The only level allowed to carry note content.
pub fn trace(message: &str) {
    log(Level::Trace, message);
}

/// A redacted description of a note collection.
///
/// Reports how many notes there are and the span they occupy, and nothing that
/// would let the log reconstruct the music.
pub fn note_summary(label: &str, count: usize, start_qn: f64, end_qn: f64) -> String {
    format!("{label}: {count} notes over [{start_qn:.3}, {end_qn:.3}] qn (content redacted)")
}

/// Replaces the panic hook with one that writes a single stderr line.
///
/// Idempotent in practice: calling it twice simply installs the same hook
/// again. It must be called before the dispatch loop starts, because a panic
/// escaping a tool must not put a Rust backtrace anywhere near stdout.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "non-string panic payload".to_string()
        };
        let location = match info.location() {
            Some(l) => format!("{}:{}:{}", l.file(), l.line(), l.column()),
            None => "unknown location".to_string(),
        };
        let line = format!(
            "{} [error] qlabs-reaper-music-mcp: PANIC at {location}: {payload}\n",
            qjson::time::now_iso8601()
        );
        let _ = std::io::stderr().write_all(line.as_bytes());
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_round_trips() {
        for l in [
            Level::Off,
            Level::Error,
            Level::Warn,
            Level::Info,
            Level::Debug,
            Level::Trace,
        ] {
            assert_eq!(Level::parse(l.id()), Some(l));
            assert_eq!(Level::from_code(l.code()), l);
        }
        assert_eq!(Level::parse("WARNING"), Some(Level::Warn));
        assert_eq!(Level::parse("nonsense"), None);
    }

    #[test]
    fn ordering_puts_trace_last() {
        assert!(Level::Off < Level::Error);
        assert!(Level::Error < Level::Trace);
    }

    #[test]
    fn note_summary_redacts_content() {
        let s = note_summary("melody", 12, 0.0, 32.0);
        assert!(s.contains("12 notes"));
        assert!(s.contains("redacted"));
        assert!(!s.contains("C4"));
    }

    #[test]
    fn set_level_takes_effect() {
        let before = level();
        set_level(Level::Off);
        assert!(!enabled(Level::Error));
        set_level(Level::Trace);
        assert!(enabled(Level::Trace));
        set_level(before);
    }
}
