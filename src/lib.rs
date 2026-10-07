//! `apimail` — HTTP API skeleton for the mail service.

pub mod config;
pub mod http;

pub use config::{AccountError, Config, ConfigError, MailAccount, MailEndpoint, TlsMode};
pub use http::{AppState, build_router};
