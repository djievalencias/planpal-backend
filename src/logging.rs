//! Application-level log filtering with five levels: DEBUG, INFO, AUDIT, WARN, ERROR.
//!
//! Set at startup with the `LOG_LEVEL` environment variable (default: INFO).
//! Change at runtime via `POST /api/v1/admin/log-level`.
//!
//! Level hierarchy (tightest → most verbose):
//!   ERROR → WARN → AUDIT → INFO → DEBUG
//!
//! Practical meaning:
//!   ERROR  – only server-side failures (5xx)
//!   WARN   – client + server errors (4xx, 5xx)
//!   AUDIT  – security events (auth, admin) + errors; suppresses routine INFO
//!   INFO   – every HTTP request + errors (default)
//!   DEBUG  – INFO + request headers + peer details

use loggix::Fields;
use serde_json::json;
use std::sync::atomic::{AtomicU8, Ordering};

static CURRENT_LEVEL: AtomicU8 = AtomicU8::new(Level::Info as u8);

/// Application log level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Level {
    Debug = 0,
    Info  = 1,
    Audit = 2,
    Warn  = 3,
    Error = 4,
}

impl Level {
    pub fn from_str(s: &str) -> Self {
        match s.to_uppercase().as_str() {
            "DEBUG" => Self::Debug,
            "AUDIT" => Self::Audit,
            "WARN"  => Self::Warn,
            "ERROR" => Self::Error,
            _       => Self::Info,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Debug => "DEBUG",
            Self::Info  => "INFO",
            Self::Audit => "AUDIT",
            Self::Warn  => "WARN",
            Self::Error => "ERROR",
        }
    }
}

/// Read `LOG_LEVEL` env var and set the level. Call once at startup.
pub fn init_from_env() {
    // Let loggix pass everything through — we own the filter.
    loggix::set_level(loggix::Level::Trace);

    let raw = std::env::var("LOG_LEVEL").unwrap_or_default();
    let level = Level::from_str(&raw);
    CURRENT_LEVEL.store(level as u8, Ordering::Relaxed);
    emit_raw(Level::Info, &[("log_level", level.as_str())], "logging initialised");
}

/// Atomically update the active log level. Safe to call from any thread.
pub fn set_level(level: Level) -> Level {
    let prev_u8 = CURRENT_LEVEL.swap(level as u8, Ordering::Relaxed);
    let prev = level_from_u8(prev_u8);
    emit_raw(
        Level::Info,
        &[
            ("previous", prev.as_str()),
            ("new_level", level.as_str()),
        ],
        "log level changed",
    );
    prev
}

/// Return the current active level.
pub fn current_level() -> Level {
    level_from_u8(CURRENT_LEVEL.load(Ordering::Relaxed))
}

/// True when `level` is at or above the current minimum.
#[inline]
pub fn is_enabled(level: Level) -> bool {
    (level as u8) >= CURRENT_LEVEL.load(Ordering::Relaxed)
}

// ── Public helpers ────────────────────────────────────────────────────────────

pub fn debug(msg: &str) {
    if is_enabled(Level::Debug) {
        emit_raw(Level::Debug, &[], msg);
    }
}

pub fn debug_with(fields: &[(&str, &str)], msg: &str) {
    if is_enabled(Level::Debug) {
        emit_raw(Level::Debug, fields, msg);
    }
}

pub fn info(msg: &str) {
    if is_enabled(Level::Info) {
        emit_raw(Level::Info, &[], msg);
    }
}

pub fn info_with(fields: &[(&str, &str)], msg: &str) {
    if is_enabled(Level::Info) {
        emit_raw(Level::Info, fields, msg);
    }
}

/// Emit an AUDIT-level log entry. Always attach structured fields for the
/// security event context (e.g. user_id, action, ip).
pub fn audit(fields: &[(&str, &str)], msg: &str) {
    if is_enabled(Level::Audit) {
        emit_raw(Level::Audit, fields, msg);
    }
}

pub fn warn(msg: &str) {
    if is_enabled(Level::Warn) {
        emit_raw(Level::Warn, &[], msg);
    }
}

pub fn warn_with(fields: &[(&str, &str)], msg: &str) {
    if is_enabled(Level::Warn) {
        emit_raw(Level::Warn, fields, msg);
    }
}

pub fn error(msg: &str) {
    if is_enabled(Level::Error) {
        emit_raw(Level::Error, &[], msg);
    }
}

pub fn error_with(fields: &[(&str, &str)], msg: &str) {
    if is_enabled(Level::Error) {
        emit_raw(Level::Error, fields, msg);
    }
}

/// For use by the HTTP logger middleware only.
pub fn emit_for_middleware(level: Level, fields: &[(&str, &str)], msg: &str) {
    emit_raw(level, fields, msg);
}

// ── Internal ──────────────────────────────────────────────────────────────────

fn level_from_u8(v: u8) -> Level {
    match v {
        0 => Level::Debug,
        1 => Level::Info,
        2 => Level::Audit,
        3 => Level::Warn,
        _ => Level::Error,
    }
}

/// Build a loggix `Fields` map, add an `audit=true` marker when needed, then
/// dispatch to the appropriate loggix level call.
fn emit_raw(level: Level, fields: &[(&str, &str)], msg: &str) {
    let mut f: Fields = Fields::new();
    for (k, v) in fields {
        f.insert(k.to_string(), json!(v));
    }
    if level == Level::Audit {
        f.insert("audit".to_string(), json!(true));
    }

    let entry = loggix::with_fields(f);
    let _ = match level {
        Level::Debug        => entry.debug(msg),
        Level::Info         => entry.info(msg),
        Level::Audit        => entry.info(msg),   // loggix has no AUDIT; INFO + audit=true field
        Level::Warn         => entry.warn(msg),
        Level::Error        => entry.error(msg),
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Guards all tests that read or write `CURRENT_LEVEL` so they never race.
    static LEVEL_LOCK: Mutex<()> = Mutex::new(());

    /// Lock `LEVEL_LOCK`, set the level to `level`, run `f`, then restore the
    /// previous level before releasing the lock.
    fn with_level<F: FnOnce()>(level: Level, f: F) {
        let _guard = LEVEL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = CURRENT_LEVEL.swap(level as u8, Ordering::Relaxed);
        f();
        CURRENT_LEVEL.store(prev, Ordering::Relaxed);
    }

    // ── Level::from_str ───────────────────────────────────────────────────────

    #[test]
    fn from_str_debug_lowercase() {
        assert_eq!(Level::from_str("debug"), Level::Debug);
    }

    #[test]
    fn from_str_debug_uppercase() {
        assert_eq!(Level::from_str("DEBUG"), Level::Debug);
    }

    #[test]
    fn from_str_info_mixed_case() {
        assert_eq!(Level::from_str("Info"), Level::Info);
    }

    #[test]
    fn from_str_audit_uppercase() {
        assert_eq!(Level::from_str("AUDIT"), Level::Audit);
    }

    #[test]
    fn from_str_warn_lowercase() {
        assert_eq!(Level::from_str("warn"), Level::Warn);
    }

    #[test]
    fn from_str_error_uppercase() {
        assert_eq!(Level::from_str("ERROR"), Level::Error);
    }

    #[test]
    fn from_str_unknown_defaults_to_info() {
        assert_eq!(Level::from_str("VERBOSE"), Level::Info);
        assert_eq!(Level::from_str(""), Level::Info);
        assert_eq!(Level::from_str("trace"), Level::Info);
    }

    // ── Level::as_str ─────────────────────────────────────────────────────────

    #[test]
    fn as_str_round_trips_with_from_str() {
        for &level in &[
            Level::Debug,
            Level::Info,
            Level::Audit,
            Level::Warn,
            Level::Error,
        ] {
            assert_eq!(Level::from_str(level.as_str()), level);
        }
    }

    #[test]
    fn as_str_values() {
        assert_eq!(Level::Debug.as_str(), "DEBUG");
        assert_eq!(Level::Info.as_str(),  "INFO");
        assert_eq!(Level::Audit.as_str(), "AUDIT");
        assert_eq!(Level::Warn.as_str(),  "WARN");
        assert_eq!(Level::Error.as_str(), "ERROR");
    }

    // ── Level ordering ────────────────────────────────────────────────────────

    #[test]
    fn level_ordering() {
        assert!(Level::Debug < Level::Info);
        assert!(Level::Info  < Level::Audit);
        assert!(Level::Audit < Level::Warn);
        assert!(Level::Warn  < Level::Error);
    }

    // ── is_enabled ────────────────────────────────────────────────────────────

    #[test]
    fn is_enabled_when_level_is_debug() {
        with_level(Level::Debug, || {
            assert!(is_enabled(Level::Debug));
            assert!(is_enabled(Level::Info));
            assert!(is_enabled(Level::Audit));
            assert!(is_enabled(Level::Warn));
            assert!(is_enabled(Level::Error));
        });
    }

    #[test]
    fn is_enabled_when_level_is_info() {
        with_level(Level::Info, || {
            assert!(!is_enabled(Level::Debug));
            assert!( is_enabled(Level::Info));
            assert!( is_enabled(Level::Audit));
            assert!( is_enabled(Level::Warn));
            assert!( is_enabled(Level::Error));
        });
    }

    #[test]
    fn is_enabled_when_level_is_audit() {
        with_level(Level::Audit, || {
            assert!(!is_enabled(Level::Debug));
            assert!(!is_enabled(Level::Info));
            assert!( is_enabled(Level::Audit));
            assert!( is_enabled(Level::Warn));
            assert!( is_enabled(Level::Error));
        });
    }

    #[test]
    fn is_enabled_when_level_is_warn() {
        with_level(Level::Warn, || {
            assert!(!is_enabled(Level::Debug));
            assert!(!is_enabled(Level::Info));
            assert!(!is_enabled(Level::Audit));
            assert!( is_enabled(Level::Warn));
            assert!( is_enabled(Level::Error));
        });
    }

    #[test]
    fn is_enabled_when_level_is_error() {
        with_level(Level::Error, || {
            assert!(!is_enabled(Level::Debug));
            assert!(!is_enabled(Level::Info));
            assert!(!is_enabled(Level::Audit));
            assert!(!is_enabled(Level::Warn));
            assert!( is_enabled(Level::Error));
        });
    }

    // ── set_level ─────────────────────────────────────────────────────────────

    #[test]
    fn set_level_returns_previous_level() {
        with_level(Level::Info, || {
            let prev = set_level(Level::Warn);
            assert_eq!(prev, Level::Info);
            // restore so with_level restores correctly
            set_level(Level::Info);
        });
    }

    #[test]
    fn set_level_changes_current_level() {
        with_level(Level::Info, || {
            set_level(Level::Error);
            assert_eq!(current_level(), Level::Error);
            set_level(Level::Info);
        });
    }

    // ── current_level ─────────────────────────────────────────────────────────

    #[test]
    fn current_level_matches_what_was_set() {
        with_level(Level::Audit, || {
            assert_eq!(current_level(), Level::Audit);
        });
    }

    // ── init_from_env ─────────────────────────────────────────────────────────

    #[test]
    fn init_from_env_defaults_to_info_when_unset() {
        with_level(Level::Error, || {
            // Ensure the var is absent.
            std::env::remove_var("LOG_LEVEL");
            init_from_env();
            assert_eq!(current_level(), Level::Info);
        });
    }

    #[test]
    fn init_from_env_sets_warn_when_env_is_warn() {
        with_level(Level::Info, || {
            std::env::set_var("LOG_LEVEL", "WARN");
            init_from_env();
            assert_eq!(current_level(), Level::Warn);
            std::env::remove_var("LOG_LEVEL");
        });
    }

    // ── Smoke tests: plain log helpers ────────────────────────────────────────

    #[test]
    fn debug_does_not_panic() {
        with_level(Level::Debug, || {
            debug("debug smoke test");
        });
    }

    #[test]
    fn info_does_not_panic() {
        with_level(Level::Info, || {
            info("info smoke test");
        });
    }

    #[test]
    fn audit_does_not_panic() {
        with_level(Level::Audit, || {
            audit(&[("user_id", "42"), ("action", "login")], "audit smoke test");
        });
    }

    #[test]
    fn warn_does_not_panic() {
        with_level(Level::Warn, || {
            warn("warn smoke test");
        });
    }

    #[test]
    fn error_does_not_panic() {
        with_level(Level::Error, || {
            error("error smoke test");
        });
    }

    // ── Smoke tests: _with helpers ────────────────────────────────────────────

    #[test]
    fn debug_with_does_not_panic() {
        with_level(Level::Debug, || {
            debug_with(&[("key", "value")], "debug_with smoke test");
        });
    }

    #[test]
    fn info_with_does_not_panic() {
        with_level(Level::Info, || {
            info_with(&[("key", "value")], "info_with smoke test");
        });
    }

    #[test]
    fn warn_with_does_not_panic() {
        with_level(Level::Warn, || {
            warn_with(&[("key", "value")], "warn_with smoke test");
        });
    }

    #[test]
    fn error_with_does_not_panic() {
        with_level(Level::Error, || {
            error_with(&[("key", "value")], "error_with smoke test");
        });
    }
}
