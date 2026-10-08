//! `apimail` — HTTP API skeleton for the mail service.

pub mod config;
pub mod http;
pub mod imap;
pub mod smtp;

pub use config::{AccountError, Config, ConfigError, MailAccount, MailEndpoint, TlsMode};
pub use http::{AppState, AppStateError, build_router};
pub use imap::{
    Address, Backoff, ConnectionManager, FetchFormat, ImapConnector, ImapError, ImapSession,
    ImapStream, MailboxInfo, MailboxStatus, Message, MessageEnvelope, MessagePage, QueryError,
    SearchCriteria, SearchDate, SendFuture, TokioImapConnector,
};
pub use smtp::{MailSender, MessageError, OutgoingAttachment, OutgoingMessage, SmtpError};
