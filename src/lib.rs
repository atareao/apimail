//! `apimail` — HTTP API skeleton for the mail service.

pub mod config;
pub mod http;
pub mod imap;
pub mod mime;
pub mod smtp;

pub use config::{AccountError, Config, ConfigError, MailAccount, MailEndpoint, TlsMode};
pub use http::{AppState, AppStateError, build_router};
pub use imap::{
    Address, Backoff, Capabilities, ConnectionManager, FetchFormat, FlagError, FlagQuery,
    ImapConnector, ImapError, ImapSession, ImapStream, MailboxInfo, MailboxStatus, Message,
    MessageEnvelope, MessagePage, ParsedMessageError, QueryError, SearchCriteria, SearchDate,
    SendFuture, SystemFlag, TokioImapConnector,
};
pub use mime::{MimeError, ParsedAttachment, ParsedMessage};
pub use smtp::{MailSender, MessageError, OutgoingAttachment, OutgoingMessage, SmtpError};
