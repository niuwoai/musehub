//! Shared version source, configuration and error contract for MuseHub.

use std::env::VarError;
use std::net::SocketAddr;
use std::str::FromStr;

const VERSION_SOURCE: &str = include_str!("../../../VERSION");

/// Environment variable that holds the HTTP listen address.
pub const BIND_ENV: &str = "MUSEHUB_BIND";

/// Listen address used when [`BIND_ENV`] is unset or blank.
pub const DEFAULT_BIND: &str = "127.0.0.1:8080";

/// Response header carrying the application version on every response.
pub const VERSION_HEADER: &str = "X-App-Version";

/// The application version, read from the repository root `VERSION` file.
pub fn version() -> &'static str {
    VERSION_SOURCE.trim()
}

/// Stable, machine readable error code for public boundaries.
pub trait ErrorCode {
    fn code(&self) -> &'static str;
}

/// Process configuration derived from the environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    bind: SocketAddr,
}

impl Config {
    /// Reads [`BIND_ENV`] from the process environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        let value = match std::env::var(BIND_ENV) {
            Ok(value) => value,
            Err(VarError::NotPresent) => return Self::from_bind_value(None),
            Err(VarError::NotUnicode(_)) => {
                return Err(ConfigError::InvalidBind {
                    value: "<non-unicode>".to_string(),
                    reason: "value is not valid unicode".to_string(),
                })
            }
        };
        Self::from_bind_value(Some(&value))
    }

    /// Parses a listen address, falling back to [`DEFAULT_BIND`] when absent.
    pub fn from_bind_value(value: Option<&str>) -> Result<Self, ConfigError> {
        let raw = match value.map(str::trim) {
            Some(value) if !value.is_empty() => value,
            _ => DEFAULT_BIND,
        };
        let bind = SocketAddr::from_str(raw).map_err(|error| ConfigError::InvalidBind {
            value: raw.to_string(),
            reason: error.to_string(),
        })?;
        Ok(Self { bind })
    }

    /// The validated listen address.
    pub fn bind(self) -> SocketAddr {
        self.bind
    }
}

/// Configuration failures. Every variant maps to the `invalid_config` code.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("invalid {BIND_ENV} value {value:?}: {reason}")]
    InvalidBind { value: String, reason: String },
}

impl ErrorCode for ConfigError {
    fn code(&self) -> &'static str {
        match self {
            Self::InvalidBind { .. } => "invalid_config",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_file_matches_package_version() {
        assert_eq!(version(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn version_file_is_a_single_line_with_trailing_newline() {
        assert!(VERSION_SOURCE.ends_with('\n'));
        assert_eq!(VERSION_SOURCE.lines().count(), 1);
        assert_eq!(VERSION_SOURCE, format!("{}\n", version()));
    }

    #[test]
    fn version_is_a_valid_header_value() {
        let value = version();
        assert!(value
            .chars()
            .all(|c| c.is_ascii_graphic() && !c.is_ascii_control()));
    }

    #[test]
    fn bind_falls_back_to_default() {
        assert_eq!(
            Config::from_bind_value(None).unwrap().bind().to_string(),
            DEFAULT_BIND
        );
        assert_eq!(
            Config::from_bind_value(Some("   "))
                .unwrap()
                .bind()
                .to_string(),
            DEFAULT_BIND
        );
    }

    #[test]
    fn bind_accepts_explicit_address() {
        let config = Config::from_bind_value(Some(" 0.0.0.0:9090 ")).unwrap();
        assert_eq!(config.bind().to_string(), "0.0.0.0:9090");
    }

    #[test]
    fn invalid_bind_reports_actionable_error() {
        let error = Config::from_bind_value(Some("not-an-address")).unwrap_err();
        assert_eq!(error.code(), "invalid_config");
        let message = error.to_string();
        assert!(message.contains(BIND_ENV), "{message}");
        assert!(message.contains("not-an-address"), "{message}");
    }

    #[test]
    fn missing_port_is_rejected() {
        let error = Config::from_bind_value(Some("127.0.0.1")).unwrap_err();
        assert_eq!(error.code(), "invalid_config");
    }
}
