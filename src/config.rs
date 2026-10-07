//! Application configuration loaded from the environment.

/// Default bind host when `APIMAIL_HOST` is unset.
pub const DEFAULT_HOST: &str = "0.0.0.0";
/// Default bind port when `APIMAIL_PORT` is unset.
pub const DEFAULT_PORT: u16 = 3000;

/// Runtime configuration for the HTTP server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Host address the server binds to.
    pub host: String,
    /// Port the server binds to.
    pub port: u16,
}

/// Errors produced while loading [`Config`].
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// `APIMAIL_PORT` was present but not a valid `u16`.
    #[error("invalid APIMAIL_PORT value `{value}`: expected a number between 0 and 65535")]
    InvalidPort {
        /// The offending raw value.
        value: String,
    },
}

impl Config {
    /// Loads configuration from the process environment.
    ///
    /// Defaults to host `0.0.0.0` and port `3000`. An invalid `APIMAIL_PORT`
    /// produces [`ConfigError::InvalidPort`] instead of a silent fallback.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Loads configuration from an arbitrary key/value source.
    ///
    /// The `lookup` closure receives the environment variable name
    /// (`APIMAIL_HOST` or `APIMAIL_PORT`) and returns its value, if present.
    /// This keeps the parsing/validation logic testable without touching the
    /// process environment.
    pub fn from_lookup<F>(lookup: F) -> Result<Self, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let host = lookup("APIMAIL_HOST").unwrap_or_else(|| DEFAULT_HOST.to_string());
        let port = match lookup("APIMAIL_PORT") {
            Some(raw) => raw
                .parse::<u16>()
                .map_err(|_| ConfigError::InvalidPort { value: raw })?,
            None => DEFAULT_PORT,
        };
        Ok(Self { host, port })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    /// Builds a lookup closure over a fixed map — no process environment, no
    /// lock, fully parallelizable.
    fn lookup_from<'a>(
        entries: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = entries
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |key| map.get(key).cloned()
    }

    #[test]
    fn defaults_when_env_absent() {
        let config = Config::from_lookup(lookup_from([])).expect("defaults should load");
        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.port, 3000);
    }

    #[test]
    fn overrides_when_env_valid() {
        let config = Config::from_lookup(lookup_from([
            ("APIMAIL_HOST", "127.0.0.1"),
            ("APIMAIL_PORT", "8080"),
        ]))
        .expect("valid overrides should load");
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 8080);
    }

    #[test]
    fn invalid_port_is_an_error() {
        let err = Config::from_lookup(lookup_from([("APIMAIL_PORT", "abc")]))
            .expect_err("invalid port must fail");
        match err {
            ConfigError::InvalidPort { value } => assert_eq!(value, "abc"),
        }
    }
}
