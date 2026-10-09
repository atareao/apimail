//! `apimail` — HTTP API skeleton for the mail service.

pub mod config;
pub mod http;
pub mod idle;
pub mod imap;
pub mod mime;
pub mod queue;
pub mod smtp;

pub use config::{AccountError, Config, ConfigError, MailAccount, MailEndpoint, TlsMode};
pub use http::{AppState, AppStateError, build_router};
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
pub use mime::{MimeError, ParsedAttachment, ParsedMessage};
pub use queue::{
    NotificationQueue, QueueError, QueueLimits, QueueStats, QueuedNotification, StoredWatermark,
    WebhookQueue,
};
pub use smtp::{MailSender, MessageError, OutgoingAttachment, OutgoingMessage, SmtpError};
