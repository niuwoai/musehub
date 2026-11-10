//! Shared version source, configuration and error contract for MuseHub.

use std::env::VarError;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;

const VERSION_SOURCE: &str = include_str!("../../../VERSION");

/// Environment variable that holds the HTTP listen address.
pub const BIND_ENV: &str = "MUSEHUB_BIND";

/// Environment variable that holds the PostgreSQL connection URL.
pub const DATABASE_URL_ENV: &str = "DATABASE_URL";

/// Root directory for bare Git repositories (`<storage_key>.git`).
pub const GIT_ROOT_ENV: &str = "MUSEHUB_GIT_ROOT";

/// Secret for signing session cookies (required when the API is enabled).
pub const SESSION_SECRET_ENV: &str = "MUSEHUB_SESSION_SECRET";

/// One-time bootstrap password for the default org admin (optional).
pub const BOOTSTRAP_ADMIN_PASSWORD_ENV: &str = "MUSEHUB_BOOTSTRAP_ADMIN_PASSWORD";

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
#[derive(Clone, PartialEq, Eq)]
pub struct Config {
    bind: SocketAddr,
    database_url: Option<String>,
    git_root: Option<PathBuf>,
    session_secret: Option<String>,
    bootstrap_admin_password: Option<String>,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Config")
            .field("bind", &self.bind)
            .field(
                "database_url",
                &self.database_url.as_ref().map(|_| "<configured>"),
            )
            .field("git_root", &self.git_root)
            .field(
                "session_secret",
                &self.session_secret.as_ref().map(|_| "<configured>"),
            )
            .field(
                "bootstrap_admin_password",
                &self
                    .bootstrap_admin_password
                    .as_ref()
                    .map(|_| "<configured>"),
            )
            .finish()
    }
}

impl Config {
    /// Reads [`BIND_ENV`] and [`DATABASE_URL_ENV`] from the process environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        let bind = match std::env::var(BIND_ENV) {
            Ok(value) => Some(value),
            Err(VarError::NotPresent) => None,
            Err(VarError::NotUnicode(_)) => {
                return Err(ConfigError::InvalidBind {
                    value: "<non-unicode>".to_string(),
                    reason: "value is not valid unicode".to_string(),
                })
            }
        };
        let config = Self::from_bind_value(bind.as_deref())?;
        let database_url = match std::env::var(DATABASE_URL_ENV) {
            Ok(value) if !value.trim().is_empty() => Some(value.trim().to_owned()),
            Ok(_) | Err(VarError::NotPresent) => None,
            Err(VarError::NotUnicode(_)) => return Err(ConfigError::InvalidDatabaseUrl),
        };
        let git_root = optional_nonempty_path(GIT_ROOT_ENV)?;
        let session_secret = optional_nonempty_string(SESSION_SECRET_ENV)?;
        let bootstrap_admin_password = optional_nonempty_string(BOOTSTRAP_ADMIN_PASSWORD_ENV)?;
        Ok(Self {
            bind: config.bind,
            database_url,
            git_root,
            session_secret,
            bootstrap_admin_password,
        })
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
        Ok(Self {
            bind,
            database_url: None,
            git_root: None,
            session_secret: None,
            bootstrap_admin_password: None,
        })
    }

    /// The validated listen address.
    pub fn bind(&self) -> SocketAddr {
        self.bind
    }

    /// The configured PostgreSQL URL, if present.
    pub fn database_url(&self) -> Option<&str> {
        self.database_url.as_deref()
    }

    /// Bare repository storage root, if configured.
    pub fn git_root(&self) -> Option<&PathBuf> {
        self.git_root.as_ref()
    }

    /// Session signing secret for cookies.
    pub fn session_secret(&self) -> Option<&str> {
        self.session_secret.as_deref()
    }

    /// Optional password used once to seed the default admin.
    pub fn bootstrap_admin_password(&self) -> Option<&str> {
        self.bootstrap_admin_password.as_deref()
    }

    /// Whether product API routes should be active.
    pub fn product_api_enabled(&self) -> bool {
        self.database_url.is_some() && self.git_root.is_some() && self.session_secret.is_some()
    }
}

fn optional_nonempty_string(name: &str) -> Result<Option<String>, ConfigError> {
    match std::env::var(name) {
        Ok(value) if value.trim().is_empty() => Ok(None),
        Ok(value) => Ok(Some(value.trim().to_owned())),
        Err(VarError::NotPresent) => Ok(None),
        Err(VarError::NotUnicode(_)) => Err(ConfigError::InvalidEnv {
            variable: name.to_string(),
            reason: "value is not valid unicode".to_string(),
        }),
    }
}

fn optional_nonempty_path(name: &str) -> Result<Option<PathBuf>, ConfigError> {
    match std::env::var(name) {
        Ok(value) if value.trim().is_empty() => Ok(None),
        Ok(value) => Ok(Some(PathBuf::from(value.trim()))),
        Err(VarError::NotPresent) => Ok(None),
        Err(VarError::NotUnicode(_)) => Err(ConfigError::InvalidEnv {
            variable: name.to_string(),
            reason: "value is not valid unicode".to_string(),
        }),
    }
}

/// Configuration failures. Every variant maps to the `invalid_config` code.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("invalid {BIND_ENV} value {value:?}: {reason}")]
    InvalidBind { value: String, reason: String },
    #[error("{DATABASE_URL_ENV} must be valid unicode")]
    InvalidDatabaseUrl,
    #[error("invalid {variable} value: {reason}")]
    InvalidEnv { variable: String, reason: String },
}

impl ErrorCode for ConfigError {
    fn code(&self) -> &'static str {
        match self {
            Self::InvalidBind { .. } | Self::InvalidDatabaseUrl | Self::InvalidEnv { .. } => {
                "invalid_config"
            }
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
