//! `apimail` — HTTP API skeleton for the mail service.

pub mod config;
pub mod http;
pub mod imap;
pub mod smtp;

pub use config::{AccountError, Config, ConfigError, MailAccount, MailEndpoint, TlsMode};
pub use http::{AppState, AppStateError, build_router};
pub use imap::{
    Backoff, ConnectionManager, ImapConnector, ImapError, ImapSession, ImapStream, SendFuture,
    TokioImapConnector,
};
pub use smtp::{MailSender, MessageError, OutgoingAttachment, OutgoingMessage, SmtpError};
