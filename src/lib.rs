//! `apimail` — an HTTP API built on Axum for a full CRUD over a mail account: read,
//! flag and search messages over IMAP, parse MIME bodies (plain text, HTML and
//! attachments), send outgoing mail over SMTP, and stream new-mail notifications
//! with IDLE and an at-least-once webhook backed by a durable queue.

pub mod config;
pub mod http;
pub mod idle;
pub mod imap;
#[cfg(feature = "mcp")]
pub mod mcp;
pub mod mime;
pub mod queue;
pub mod service;
pub mod smtp;

pub use config::{AccountError, Config, ConfigError, MailAccount, MailEndpoint, TlsMode};
pub use http::build_router;
pub use idle::{
    AddressPayload, AttachmentPayload, EnvelopePayload, HttpWebhookSender, IDLE_REISSUE_INTERVAL,
    IdleConnector, IdleError, IdleEvent, IdleSession, IdleSettings, IdleStatus, IdleSupervisor,
    LAST_ERROR_IMAP_UNAVAILABLE, LAST_ERROR_WEBHOOK_FAILED, STATUS_RUNNING, STATUS_STOPPED,
    STOP_TIMEOUT, TokioIdleConnector, WebhookError, WebhookPayload, WebhookSender,
};
pub use imap::{
    Address, Backoff, Capabilities, ConnectionManager, FetchFormat, FlagError, FlagQuery,
    ImapConnector, ImapError, ImapSession, ImapStream, MailboxInfo, MailboxStatus, Message,
    MessageEnvelope, MessagePage, ParsedMessageError, QueryError, SearchCriteria, SearchDate,
    SendFuture, SystemFlag, TokioImapConnector,
};
#[cfg(feature = "mcp")]
pub use mcp::{ApimailMcp, McpServeError};
pub use mime::{MimeError, ParsedAttachment, ParsedMessage};
pub use queue::{
    NotificationQueue, QueueError, QueueLimits, QueueStats, QueuedNotification, StoredWatermark,
    WebhookQueue,
};
pub use service::{AppState, AppStateError, ServiceError};
pub use smtp::{MailSender, MessageError, OutgoingAttachment, OutgoingMessage, SmtpError};
