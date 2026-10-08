//! Application configuration loaded from the environment.

use std::time::Duration;

/// Default bind host when `APIMAIL_HOST` is unset.
pub const DEFAULT_HOST: &str = "0.0.0.0";
/// Default bind port when `APIMAIL_PORT` is unset.
pub const DEFAULT_PORT: u16 = 3000;
/// Environment variable holding the maximum total attachment size, in bytes.
pub const MAX_ATTACHMENT_BYTES_VAR: &str = "APIMAIL_MAX_ATTACHMENT_BYTES";
/// Default maximum total attachment size (10 MiB) when
/// `APIMAIL_MAX_ATTACHMENT_BYTES` is unset.
pub const DEFAULT_MAX_ATTACHMENT_BYTES: usize = 10 * 1024 * 1024;
/// Environment variable holding the IMAP operation timeout, in seconds.
pub const IMAP_TIMEOUT_SECS_VAR: &str = "APIMAIL_IMAP_TIMEOUT_SECS";
/// Default IMAP operation timeout (30 s) when `APIMAIL_IMAP_TIMEOUT_SECS` is
/// unset.
pub const DEFAULT_IMAP_TIMEOUT_SECS: u64 = 30;

/// TLS mode negotiated with a mail endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsMode {
    /// Implicit TLS: the connection is encrypted from the very first byte.
    Implicit,
    /// Cleartext connection upgraded in-band via the `STARTTLS` command.
    StartTls,
    /// No TLS at all (allowed with a warning, for local testing servers).
    Plain,
}

impl TlsMode {
    /// Stable, lower-case label used in API responses.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Implicit => "implicit",
            Self::StartTls => "starttls",
            Self::Plain => "none",
        }
    }

    /// Parses a TLS mode case-insensitively, or `None` when unsupported.
    fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "implicit" => Some(Self::Implicit),
            "starttls" => Some(Self::StartTls),
            "none" => Some(Self::Plain),
            _ => None,
        }
    }

    /// Default port for this TLS mode on the given protocol.
    fn default_port(self, protocol: Protocol) -> u16 {
        match (protocol, self) {
            (Protocol::Imap, TlsMode::Implicit) => 993,
            (Protocol::Imap, TlsMode::StartTls | TlsMode::Plain) => 143,
            (Protocol::Smtp, TlsMode::Implicit) => 465,
            (Protocol::Smtp, TlsMode::StartTls | TlsMode::Plain) => 587,
        }
    }
}

/// Mail protocol a [`MailEndpoint`] targets; used to pick the default port.
#[derive(Clone, Copy)]
enum Protocol {
    /// Incoming mail access (IMAP).
    Imap,
    /// Outgoing mail submission (SMTP).
    Smtp,
}

/// A single mail endpoint (IMAP or SMTP).
#[derive(Clone)]
pub struct MailEndpoint {
    /// Server host name or address.
    pub host: String,
    /// Server port.
    pub port: u16,
    /// TLS mode negotiated with the server.
    pub tls: TlsMode,
    /// Account user name.
    pub username: String,
    /// Account password; redacted in [`Debug`](std::fmt::Debug) output.
    pub password: String,
}

impl std::fmt::Debug for MailEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MailEndpoint")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("tls", &self.tls)
            .field("username", &self.username)
            .field("password", &"***")
            .finish()
    }
}

/// The mail account: one IMAP endpoint and one SMTP endpoint.
#[derive(Debug, Clone)]
pub struct MailAccount {
    /// Incoming (IMAP) endpoint.
    pub imap: MailEndpoint,
    /// Outgoing (SMTP) endpoint.
    pub smtp: MailEndpoint,
}

impl MailAccount {
    /// Loads the mail account from an arbitrary key/value source.
    ///
    /// Both the IMAP (`APIMAIL_IMAP_*`) and the SMTP (`APIMAIL_SMTP_*`) endpoints
    /// require a non-empty `HOST`, `USER` and `PASSWORD`. `PORT` and `TLS` are
    /// optional; a missing port is derived from the TLS mode (which itself
    /// defaults to `implicit`). This keeps the parsing/validation testable
    /// without touching the process environment.
    pub fn from_lookup<F>(lookup: &F) -> Result<Self, AccountError>
    where
        F: Fn(&str) -> Option<String>,
    {
        Ok(Self {
            imap: load_endpoint(lookup, "APIMAIL_IMAP", Protocol::Imap)?,
            smtp: load_endpoint(lookup, "APIMAIL_SMTP", Protocol::Smtp)?,
        })
    }
}

/// Errors produced while loading a [`MailAccount`].
#[derive(Debug, thiserror::Error)]
pub enum AccountError {
    /// A required variable was absent or empty.
    #[error("missing {name}: a non-empty value is required to start the service")]
    MissingValue {
        /// Name of the offending environment variable.
        name: String,
    },
    /// A port variable was present but not a number between 1 and 65535.
    #[error("invalid {name} value `{value}`: expected a number between 1 and 65535")]
    InvalidPort {
        /// Name of the offending environment variable.
        name: String,
        /// The offending raw value.
        value: String,
    },
    /// A TLS variable held an unsupported value.
    #[error("invalid {name} value `{value}`: expected one of `implicit`, `starttls` or `none`")]
    InvalidTls {
        /// Name of the offending environment variable.
        name: String,
        /// The offending raw value.
        value: String,
    },
}

/// Reads a required, non-empty variable or fails with
/// [`AccountError::MissingValue`].
///
/// A value that is empty or whitespace-only counts as missing. A valid value is
/// returned **untrimmed**, so a password may legitimately contain spaces.
fn required<F>(lookup: &F, name: &str) -> Result<String, AccountError>
where
    F: Fn(&str) -> Option<String>,
{
    match lookup(name) {
        Some(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(AccountError::MissingValue {
            name: name.to_string(),
        }),
    }
}

/// Loads one endpoint from the variables sharing `prefix` (`HOST`, `USER`,
/// `PASSWORD`, `PORT`, `TLS`).
fn load_endpoint<F>(
    lookup: &F,
    prefix: &str,
    protocol: Protocol,
) -> Result<MailEndpoint, AccountError>
where
    F: Fn(&str) -> Option<String>,
{
    let host = required(lookup, &format!("{prefix}_HOST"))?;
    let username = required(lookup, &format!("{prefix}_USER"))?;
    let password = required(lookup, &format!("{prefix}_PASSWORD"))?;

    let tls_var = format!("{prefix}_TLS");
    let tls = match lookup(&tls_var) {
        Some(raw) => TlsMode::parse(&raw).ok_or_else(|| AccountError::InvalidTls {
            name: tls_var.clone(),
            value: raw,
        })?,
        None => TlsMode::Implicit,
    };
    if tls == TlsMode::Plain {
        tracing::warn!(
            "{tls_var} is set to `none`: TLS is disabled for {prefix} and credentials will be sent in clear text"
        );
    }

    let port_var = format!("{prefix}_PORT");
    let port = match lookup(&port_var) {
        Some(raw) => {
            let parsed = raw.parse::<u16>().map_err(|_| AccountError::InvalidPort {
                name: port_var.clone(),
                value: raw.clone(),
            })?;
            if parsed == 0 {
                return Err(AccountError::InvalidPort {
                    name: port_var,
                    value: raw,
                });
            }
            parsed
        }
        None => tls.default_port(protocol),
    };

    Ok(MailEndpoint {
        host,
        port,
        tls,
        username,
        password,
    })
}

/// Runtime configuration for the HTTP server and the mail account.
#[derive(Clone)]
pub struct Config {
    /// Host address the server binds to.
    pub host: String,
    /// Port the server binds to.
    pub port: u16,
    /// Static API key required to authenticate requests.
    pub api_key: String,
    /// Mail account the service operates on.
    pub account: MailAccount,
    /// Maximum total size, in bytes, of the decoded attachments of a message.
    pub max_attachment_bytes: usize,
    /// Timeout applied to TCP connect, the TLS handshake and the IMAP login.
    pub imap_timeout: Duration,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("api_key", &"***")
            .field("account", &self.account)
            .field("imap_timeout", &self.imap_timeout)
            .finish()
    }
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
    /// `APIMAIL_API_KEY` was absent or empty.
    #[error("missing APIMAIL_API_KEY: a non-empty API key is required to start the service")]
    MissingApiKey,
    /// `APIMAIL_MAX_ATTACHMENT_BYTES` was present but not a positive integer.
    #[error(
        "invalid APIMAIL_MAX_ATTACHMENT_BYTES value `{value}`: expected a positive number of bytes"
    )]
    InvalidMaxAttachmentBytes {
        /// The offending raw value.
        value: String,
    },
    /// `APIMAIL_IMAP_TIMEOUT_SECS` was present but not a positive integer.
    #[error(
        "invalid APIMAIL_IMAP_TIMEOUT_SECS value `{value}`: expected a positive number of seconds"
    )]
    InvalidImapTimeout {
        /// The offending raw value.
        value: String,
    },
    /// The mail account could not be loaded.
    #[error(transparent)]
    Account(#[from] AccountError),
}

impl Config {
    /// Loads configuration from the process environment.
    ///
    /// Defaults to host `0.0.0.0` and port `3000`. An invalid `APIMAIL_PORT`
    /// produces [`ConfigError::InvalidPort`] instead of a silent fallback, a
    /// missing or empty `APIMAIL_API_KEY` produces [`ConfigError::MissingApiKey`],
    /// and an incomplete mail account produces [`ConfigError::Account`].
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Loads configuration from an arbitrary key/value source.
    ///
    /// The `lookup` closure receives the environment variable name and returns
    /// its value, if present. This keeps the parsing/validation logic testable
    /// without touching the process environment.
    ///
    /// Validation runs in the order host → port → api key → mail account →
    /// attachment limit → IMAP timeout, so an invalid server port is reported
    /// before any missing API key or account value, an invalid attachment limit
    /// is reported after the account, and an invalid IMAP timeout last.
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
        let api_key = match lookup("APIMAIL_API_KEY") {
            Some(key) if !key.is_empty() => key,
            _ => return Err(ConfigError::MissingApiKey),
        };
        let account = MailAccount::from_lookup(&lookup)?;
        let max_attachment_bytes = match lookup(MAX_ATTACHMENT_BYTES_VAR) {
            Some(raw) => {
                let parsed = raw
                    .parse::<usize>()
                    .map_err(|_| ConfigError::InvalidMaxAttachmentBytes { value: raw.clone() })?;
                if parsed == 0 {
                    return Err(ConfigError::InvalidMaxAttachmentBytes { value: raw });
                }
                parsed
            }
            None => DEFAULT_MAX_ATTACHMENT_BYTES,
        };
        let imap_timeout = match lookup(IMAP_TIMEOUT_SECS_VAR) {
            Some(raw) => {
                let parsed = raw
                    .parse::<u64>()
                    .map_err(|_| ConfigError::InvalidImapTimeout { value: raw.clone() })?;
                if parsed == 0 {
                    return Err(ConfigError::InvalidImapTimeout { value: raw });
                }
                Duration::from_secs(parsed)
            }
            None => Duration::from_secs(DEFAULT_IMAP_TIMEOUT_SECS),
        };
        Ok(Self {
            host,
            port,
            api_key,
            account,
            max_attachment_bytes,
            imap_timeout,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use super::*;

    /// A [`MakeWriter`](tracing_subscriber::fmt::MakeWriter) that appends every
    /// log line to a shared buffer, so a test can assert on captured output
    /// without touching the global subscriber.
    #[derive(Clone, Default)]
    struct SharedWriter(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for SharedWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("log buffer lock poisoned")
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for SharedWriter {
        type Writer = SharedWriter;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

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

    /// Minimal valid configuration: API key plus both mail endpoints.
    fn base_entries() -> Vec<(&'static str, &'static str)> {
        vec![
            ("APIMAIL_API_KEY", "secret"),
            ("APIMAIL_IMAP_HOST", "imap.example.com"),
            ("APIMAIL_IMAP_USER", "imap-user"),
            ("APIMAIL_IMAP_PASSWORD", "imap-secret"),
            ("APIMAIL_SMTP_HOST", "smtp.example.com"),
            ("APIMAIL_SMTP_USER", "smtp-user"),
            ("APIMAIL_SMTP_PASSWORD", "smtp-secret"),
        ]
    }

    /// The six required mail-account variables.
    const REQUIRED: [&str; 6] = [
        "APIMAIL_IMAP_HOST",
        "APIMAIL_IMAP_USER",
        "APIMAIL_IMAP_PASSWORD",
        "APIMAIL_SMTP_HOST",
        "APIMAIL_SMTP_USER",
        "APIMAIL_SMTP_PASSWORD",
    ];

    #[test]
    fn defaults_when_env_absent() {
        let config =
            Config::from_lookup(lookup_from(base_entries())).expect("defaults should load");
        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.port, 3000);
    }

    #[test]
    fn overrides_when_env_valid() {
        let config = Config::from_lookup(lookup_from(
            base_entries()
                .into_iter()
                .chain([("APIMAIL_HOST", "127.0.0.1"), ("APIMAIL_PORT", "8080")]),
        ))
        .expect("valid overrides should load");
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 8080);
        assert_eq!(config.api_key, "secret");
    }

    #[test]
    fn api_key_is_loaded_when_valid() {
        let config =
            Config::from_lookup(lookup_from(base_entries())).expect("valid config should load");
        assert_eq!(config.api_key, "secret");
    }

    #[test]
    fn missing_api_key_is_an_error() {
        let err = Config::from_lookup(lookup_from([])).expect_err("missing api key must fail");
        assert!(matches!(err, ConfigError::MissingApiKey));
    }

    #[test]
    fn empty_api_key_is_an_error() {
        let err = Config::from_lookup(lookup_from([("APIMAIL_API_KEY", "")]))
            .expect_err("empty api key must fail");
        assert!(matches!(err, ConfigError::MissingApiKey));
    }

    #[test]
    fn config_debug_does_not_leak_api_key() {
        let config = Config::from_lookup(lookup_from(
            base_entries()
                .into_iter()
                .chain([("APIMAIL_API_KEY", "top-secret-key")]),
        ))
        .expect("valid config should load");
        let rendered = format!("{config:?}");
        assert!(
            !rendered.contains("top-secret-key"),
            "Debug output leaked the API key: {rendered}"
        );
        assert!(
            rendered.contains("***"),
            "Debug output is missing the redaction marker: {rendered}"
        );
    }

    #[test]
    fn invalid_port_is_an_error() {
        let err = Config::from_lookup(lookup_from([("APIMAIL_PORT", "abc")]))
            .expect_err("invalid port must fail");
        match err {
            ConfigError::InvalidPort { value } => assert_eq!(value, "abc"),
            other => panic!("expected InvalidPort, got {other:?}"),
        }
    }

    #[test]
    fn mail_account_loads_complete_configuration() {
        let account =
            MailAccount::from_lookup(&lookup_from(base_entries())).expect("complete account");
        assert_eq!(account.imap.host, "imap.example.com");
        assert_eq!(account.imap.username, "imap-user");
        assert_eq!(account.imap.password, "imap-secret");
        assert_eq!(account.smtp.host, "smtp.example.com");
        assert_eq!(account.smtp.username, "smtp-user");
        assert_eq!(account.smtp.password, "smtp-secret");
    }

    #[test]
    fn tls_defaults_to_implicit() {
        let account =
            MailAccount::from_lookup(&lookup_from(base_entries())).expect("valid account");
        assert_eq!(account.imap.tls, TlsMode::Implicit);
        assert_eq!(account.smtp.tls, TlsMode::Implicit);
    }

    #[test]
    fn tls_modes_are_parsed_case_insensitively() {
        let account = MailAccount::from_lookup(&lookup_from(base_entries().into_iter().chain([
            ("APIMAIL_IMAP_TLS", "StartTls"),
            ("APIMAIL_SMTP_TLS", "NONE"),
        ])))
        .expect("valid account");
        assert_eq!(account.imap.tls, TlsMode::StartTls);
        assert_eq!(account.smtp.tls, TlsMode::Plain);
    }

    #[test]
    fn invalid_tls_is_an_error() {
        for name in ["APIMAIL_IMAP_TLS", "APIMAIL_SMTP_TLS"] {
            let err = MailAccount::from_lookup(&lookup_from(
                base_entries().into_iter().chain([(name, "ssl")]),
            ))
            .expect_err("invalid tls must fail");
            match err {
                AccountError::InvalidTls { name: got, value } => {
                    assert_eq!(got, name);
                    assert_eq!(value, "ssl");
                }
                other => panic!("expected InvalidTls for {name}, got {other:?}"),
            }
        }
    }

    #[test]
    fn default_ports_follow_the_tls_mode() {
        let implicit =
            MailAccount::from_lookup(&lookup_from(base_entries())).expect("valid account");
        assert_eq!(implicit.imap.port, 993);
        assert_eq!(implicit.smtp.port, 465);

        let starttls = MailAccount::from_lookup(&lookup_from(base_entries().into_iter().chain([
            ("APIMAIL_IMAP_TLS", "starttls"),
            ("APIMAIL_SMTP_TLS", "starttls"),
        ])))
        .expect("valid account");
        assert_eq!(starttls.imap.port, 143);
        assert_eq!(starttls.smtp.port, 587);

        let none = MailAccount::from_lookup(&lookup_from(
            base_entries()
                .into_iter()
                .chain([("APIMAIL_IMAP_TLS", "none"), ("APIMAIL_SMTP_TLS", "none")]),
        ))
        .expect("valid account");
        assert_eq!(none.imap.port, 143);
        assert_eq!(none.smtp.port, 587);
    }

    #[test]
    fn explicit_port_overrides_the_default() {
        let account = MailAccount::from_lookup(&lookup_from(base_entries().into_iter().chain([
            ("APIMAIL_IMAP_TLS", "starttls"),
            ("APIMAIL_IMAP_PORT", "1993"),
        ])))
        .expect("valid account");
        assert_eq!(account.imap.port, 1993);
    }

    #[test]
    fn invalid_port_variants_are_an_error() {
        for value in ["abc", "0", "70000"] {
            let err = MailAccount::from_lookup(&lookup_from(
                base_entries()
                    .into_iter()
                    .chain([("APIMAIL_IMAP_PORT", value)]),
            ))
            .expect_err("invalid port must fail");
            match err {
                AccountError::InvalidPort { name, value: got } => {
                    assert_eq!(name, "APIMAIL_IMAP_PORT");
                    assert_eq!(got, value);
                }
                other => panic!("expected InvalidPort for {value}, got {other:?}"),
            }
        }
    }

    #[test]
    fn missing_required_value_is_an_error() {
        for name in REQUIRED {
            let err = MailAccount::from_lookup(&lookup_from(
                base_entries().into_iter().filter(|(key, _)| *key != name),
            ))
            .expect_err("missing value must fail");
            match err {
                AccountError::MissingValue { name: got } => assert_eq!(got, name),
                other => panic!("expected MissingValue for {name}, got {other:?}"),
            }
        }
    }

    #[test]
    fn empty_required_value_is_an_error() {
        for name in REQUIRED {
            let err = MailAccount::from_lookup(&lookup_from(
                base_entries()
                    .into_iter()
                    .map(|(key, value)| if key == name { (key, "") } else { (key, value) }),
            ))
            .expect_err("empty value must fail");
            match err {
                AccountError::MissingValue { name: got } => assert_eq!(got, name),
                other => panic!("expected MissingValue for {name}, got {other:?}"),
            }
        }
    }

    #[test]
    fn whitespace_only_required_value_is_an_error() {
        for name in REQUIRED {
            let err = MailAccount::from_lookup(&lookup_from(base_entries().into_iter().map(
                |(key, value)| {
                    if key == name {
                        (key, "   ")
                    } else {
                        (key, value)
                    }
                },
            )))
            .expect_err("whitespace-only value must fail");
            match err {
                AccountError::MissingValue { name: got } => assert_eq!(got, name),
                other => panic!("expected MissingValue for {name}, got {other:?}"),
            }
        }
    }

    #[test]
    fn valid_values_are_not_trimmed() {
        let password_with_spaces = "\tpass with spaces ";
        let account = MailAccount::from_lookup(&lookup_from(base_entries().into_iter().map(
            |(key, value)| match key {
                "APIMAIL_IMAP_PASSWORD" => (key, password_with_spaces),
                _ => (key, value),
            },
        )))
        .expect("valid account");
        assert_eq!(account.imap.password, password_with_spaces);
    }

    #[test]
    fn plaintext_mode_logs_a_warning() {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .with_writer(SharedWriter(buffer.clone()))
            .with_max_level(tracing::Level::WARN)
            .with_ansi(false)
            .finish();

        let lookup = lookup_from(
            base_entries()
                .into_iter()
                .chain([("APIMAIL_IMAP_TLS", "none"), ("APIMAIL_SMTP_TLS", "none")]),
        );

        tracing::subscriber::with_default(subscriber, || {
            MailAccount::from_lookup(&lookup).expect("valid account");
        });

        let captured = buffer.lock().expect("log buffer lock poisoned").clone();
        let output = String::from_utf8(captured).expect("captured log should be UTF-8");
        assert!(output.contains("WARN"), "expected a WARN line: {output}");
        assert!(
            output.contains("APIMAIL_IMAP_TLS"),
            "warning should mention APIMAIL_IMAP_TLS: {output}"
        );
        assert!(
            output.contains("APIMAIL_SMTP_TLS"),
            "warning should mention APIMAIL_SMTP_TLS: {output}"
        );
        assert!(
            output.contains("clear text"),
            "warning should explain the risk: {output}"
        );
    }

    #[test]
    fn mail_account_debug_redacts_passwords() {
        let account =
            MailAccount::from_lookup(&lookup_from(base_entries())).expect("valid account");
        let rendered = format!("{account:?}");
        assert!(
            !rendered.contains("imap-secret"),
            "Debug output leaked the IMAP password: {rendered}"
        );
        assert!(
            !rendered.contains("smtp-secret"),
            "Debug output leaked the SMTP password: {rendered}"
        );
        assert!(
            rendered.contains("***"),
            "Debug output is missing the redaction marker: {rendered}"
        );
    }

    #[test]
    fn config_loads_the_mail_account() {
        let config = Config::from_lookup(lookup_from(base_entries())).expect("valid config");
        assert_eq!(config.account.imap.host, "imap.example.com");
        assert_eq!(config.account.smtp.host, "smtp.example.com");
    }

    #[test]
    fn account_error_propagates_from_config() {
        let err = Config::from_lookup(lookup_from(
            base_entries()
                .into_iter()
                .filter(|(key, _)| *key != "APIMAIL_IMAP_HOST"),
        ))
        .expect_err("missing account variable must fail");
        match err {
            ConfigError::Account(AccountError::MissingValue { name }) => {
                assert_eq!(name, "APIMAIL_IMAP_HOST");
            }
            other => panic!("expected Account(MissingValue), got {other:?}"),
        }
    }

    #[test]
    fn config_debug_does_not_leak_account_passwords() {
        let config = Config::from_lookup(lookup_from(base_entries())).expect("valid config");
        let rendered = format!("{config:?}");
        assert!(
            !rendered.contains("imap-secret"),
            "Config Debug leaked the IMAP password: {rendered}"
        );
        assert!(
            !rendered.contains("smtp-secret"),
            "Config Debug leaked the SMTP password: {rendered}"
        );
    }

    #[test]
    fn max_attachment_defaults_to_ten_mib_when_absent() {
        let config = Config::from_lookup(lookup_from(base_entries())).expect("valid config");
        assert_eq!(config.max_attachment_bytes, DEFAULT_MAX_ATTACHMENT_BYTES);
        assert_eq!(DEFAULT_MAX_ATTACHMENT_BYTES, 10 * 1024 * 1024);
    }

    #[test]
    fn max_attachment_uses_configured_value() {
        let config = Config::from_lookup(lookup_from(
            base_entries()
                .into_iter()
                .chain([(MAX_ATTACHMENT_BYTES_VAR, "2048")]),
        ))
        .expect("valid config");
        assert_eq!(config.max_attachment_bytes, 2048);
    }

    #[test]
    fn max_attachment_zero_is_an_error() {
        let err = Config::from_lookup(lookup_from(
            base_entries()
                .into_iter()
                .chain([(MAX_ATTACHMENT_BYTES_VAR, "0")]),
        ))
        .expect_err("zero limit must fail");
        match err {
            ConfigError::InvalidMaxAttachmentBytes { value } => assert_eq!(value, "0"),
            other => panic!("expected InvalidMaxAttachmentBytes, got {other:?}"),
        }
    }

    #[test]
    fn max_attachment_non_numeric_is_an_error() {
        let err = Config::from_lookup(lookup_from(
            base_entries()
                .into_iter()
                .chain([(MAX_ATTACHMENT_BYTES_VAR, "abc")]),
        ))
        .expect_err("non-numeric limit must fail");
        assert!(
            err.to_string().contains(MAX_ATTACHMENT_BYTES_VAR),
            "the error message must name the offending variable: {err}"
        );
        match err {
            ConfigError::InvalidMaxAttachmentBytes { value } => assert_eq!(value, "abc"),
            other => panic!("expected InvalidMaxAttachmentBytes, got {other:?}"),
        }
    }

    #[test]
    fn imap_timeout_defaults_to_30_seconds_when_absent() {
        let config = Config::from_lookup(lookup_from(base_entries())).expect("valid config");
        assert_eq!(
            config.imap_timeout,
            Duration::from_secs(DEFAULT_IMAP_TIMEOUT_SECS)
        );
        assert_eq!(DEFAULT_IMAP_TIMEOUT_SECS, 30);
    }

    #[test]
    fn imap_timeout_uses_configured_value() {
        let config = Config::from_lookup(lookup_from(
            base_entries()
                .into_iter()
                .chain([(IMAP_TIMEOUT_SECS_VAR, "5")]),
        ))
        .expect("valid config");
        assert_eq!(config.imap_timeout, Duration::from_secs(5));
    }

    #[test]
    fn imap_timeout_zero_is_an_error() {
        let err = Config::from_lookup(lookup_from(
            base_entries()
                .into_iter()
                .chain([(IMAP_TIMEOUT_SECS_VAR, "0")]),
        ))
        .expect_err("zero timeout must fail");
        assert!(
            err.to_string().contains(IMAP_TIMEOUT_SECS_VAR),
            "the error message must name the offending variable: {err}"
        );
        match err {
            ConfigError::InvalidImapTimeout { value } => assert_eq!(value, "0"),
            other => panic!("expected InvalidImapTimeout, got {other:?}"),
        }
    }

    #[test]
    fn imap_timeout_non_numeric_is_an_error() {
        let err = Config::from_lookup(lookup_from(
            base_entries()
                .into_iter()
                .chain([(IMAP_TIMEOUT_SECS_VAR, "abc")]),
        ))
        .expect_err("non-numeric timeout must fail");
        assert!(
            err.to_string().contains(IMAP_TIMEOUT_SECS_VAR),
            "the error message must name the offending variable: {err}"
        );
        match err {
            ConfigError::InvalidImapTimeout { value } => assert_eq!(value, "abc"),
            other => panic!("expected InvalidImapTimeout, got {other:?}"),
        }
    }

    #[test]
    fn tls_mode_as_str_matches_the_api_labels() {
        assert_eq!(TlsMode::Implicit.as_str(), "implicit");
        assert_eq!(TlsMode::StartTls.as_str(), "starttls");
        assert_eq!(TlsMode::Plain.as_str(), "none");
    }
}
